//! Model conformance (OT-010 design D8): replays the Quint models' ITF traces
//! against the real repositories and Postgres, so a model that drifts from the
//! code fails here.
//!
//! `just models` exports random traces of `models/attempt_claim.qnt` (modules
//! `email_codes` and `reset_codes`) and `models/refresh_rotation.qnt` into
//! `MODEL_TRACES_DIR`, then runs this suite. Each model step is one SQL
//! statement (or one transaction); the replay runs it through the repository
//! method that issues it, on that request's own database connection, in the
//! trace's order. After every step it checks the call's result and the stored
//! rows against the model's next state, and names the trace and step on a
//! mismatch.
//!
//! Time: the model counts whole minutes. A model time `t` is the real instant
//! `now - (model_now - t)`, and a `tick` moves every stored timestamp back by
//! its minutes, so the database's own `NOW()` stays the model's `now`.
//!
//! Ignored by plain `cargo test`: it needs the exported traces. Run it with
//! `just models`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use application::ports::{
    EmailVerificationRepository, PasswordResetRepository, RefreshTokenRepository, UserRepository,
};
use domain::entities::attempt_lockout::{AttemptOutcome, IssueOutcome, RESEND_COOLDOWN_MINUTES};
use domain::entities::{
    EmailVerificationCode, PasswordResetCode, RefreshToken, RefreshTokenStatus, User,
    email_verification_code, password_reset_code,
};
use domain::errors::DomainError;
use domain::value_objects::{Email, RefreshTokenId, ResetCodeId, UserId, VerificationCodeId};
use jiff::{SignedDuration, Timestamp};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use server::infrastructure::persistence::postgres::{
    Database, PostgresEmailVerificationRepository, PostgresPasswordResetRepository,
    PostgresRefreshTokenRepository, PostgresUserRepository,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// Slack when comparing a stored timestamp with the one the model implies:
/// the replay itself takes real time.
const CLOCK_SLACK_SECONDS: i64 = 60;

// ── Traces ─────────────────────────────────────────────────────────────────

/// The traces `just models` exported for `prefix`, sorted by name.
fn traces(prefix: &str) -> Vec<PathBuf> {
    let dir = std::env::var("MODEL_TRACES_DIR")
        .expect("MODEL_TRACES_DIR is unset: run this suite with `just models`");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("can't read MODEL_TRACES_DIR {dir}: {e}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".itf.json"))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no {prefix}*.itf.json traces in {dir}");
    files
}

/// Reads a trace, dropping the `module::instance::` prefix Quint puts on the
/// variable names of an instantiated module.
fn load<S: DeserializeOwned>(path: &Path) -> Vec<S> {
    let text = std::fs::read_to_string(path).unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&text).unwrap();
    for state in json["states"].as_array_mut().unwrap() {
        let object = state.as_object_mut().unwrap();
        let renamed: serde_json::Map<String, serde_json::Value> = std::mem::take(object)
            .into_iter()
            .map(|(key, value)| (key.rsplit("::").next().unwrap().to_string(), value))
            .collect();
        *object = renamed;
    }
    itf::trace_from_value::<S>(json)
        .unwrap_or_else(|e| panic!("{}: not a trace of this model: {e}", path.display()))
        .states
        .into_iter()
        .map(|state| state.value)
        .collect()
}

/// The real instant for model time `t` when the model's clock reads `now`.
fn at(now: i64, t: i64) -> Timestamp {
    Timestamp::now() - SignedDuration::from_mins(now - t)
}

fn assert_near(what: &str, actual: Timestamp, expected: Timestamp) {
    let off = (actual - expected).as_secs().abs();
    assert!(
        off <= CLOCK_SLACK_SECONDS,
        "{what}: stored {actual}, model implies {expected} ({off}s apart)"
    );
}

/// One connection per request, so the requests' statements run on different
/// sessions, as they do in production.
async fn session(pool: &PgPool) -> Arc<Database> {
    let options = (*pool.connect_options()).clone();
    let single = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    Arc::new(Database::from_pool(single))
}

async fn new_user(pool: &PgPool, label: &str) -> UserId {
    let user = User::new(
        Email::new(&format!("{label}@conformance.example.com")).unwrap(),
        Some("hash".to_string()),
        "Model".to_string(),
        "Replay".to_string(),
    );
    PostgresUserRepository::new(Arc::new(Database::from_pool(pool.clone())))
        .save(&user)
        .await
        .unwrap();
    *user.id()
}

// ── attempt_claim.qnt ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct Row {
    present: bool,
    id: i64,
    attempts: i64,
    lockouts: i64,
    sent: i64,
    expires: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct CodeStep {
    act: String,
    who: i64,
    /// The guess was correct (`guessCompare`), or the email failed
    /// (`issueDeliver`).
    flag: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeState {
    row: Row,
    now: i64,
    guess_pc: BTreeMap<i64, String>,
    guess_code: BTreeMap<i64, i64>,
    guess_out: BTreeMap<i64, String>,
    issue_pc: BTreeMap<i64, String>,
    issue_code: BTreeMap<i64, i64>,
    issue_out: BTreeMap<i64, String>,
    last: CodeStep,
}

#[derive(Clone, Copy, Debug)]
enum CodeKind {
    Email,
    Reset,
}

impl CodeKind {
    const fn expiry_minutes(self) -> i64 {
        match self {
            Self::Email => email_verification_code::EXPIRY_HOURS * 60,
            Self::Reset => password_reset_code::EXPIRY_MINUTES,
        }
    }

    const fn set_lockouts(self) -> &'static str {
        match self {
            Self::Email => "UPDATE email_verification_codes SET lockouts = $2 WHERE user_id = $1",
            Self::Reset => "UPDATE password_reset_codes SET lockouts = $2 WHERE user_id = $1",
        }
    }

    const fn rewind(self) -> &'static str {
        match self {
            Self::Email => {
                "UPDATE email_verification_codes SET \
                 last_sent_at = last_sent_at - make_interval(mins => $1), \
                 expires_at = expires_at - make_interval(mins => $1), \
                 created_at = created_at - make_interval(mins => $1)"
            }
            Self::Reset => {
                "UPDATE password_reset_codes SET \
                 last_sent_at = last_sent_at - make_interval(mins => $1), \
                 expires_at = expires_at - make_interval(mins => $1), \
                 created_at = created_at - make_interval(mins => $1)"
            }
        }
    }

    const fn clear(self) -> &'static str {
        match self {
            Self::Email => "DELETE FROM email_verification_codes",
            Self::Reset => "DELETE FROM password_reset_codes",
        }
    }
}

/// A stored code as the replay needs it.
struct Found {
    id: Uuid,
    attempts: i32,
    lockouts: i32,
    sent: Timestamp,
    expires: Timestamp,
    expired: bool,
    reissue_allowed: bool,
}

/// One request's view of a code repository, on its own connection.
struct Codes {
    kind: CodeKind,
    email: PostgresEmailVerificationRepository,
    reset: PostgresPasswordResetRepository,
}

impl Codes {
    async fn new(kind: CodeKind, pool: &PgPool) -> Self {
        let db = session(pool).await;
        Self {
            kind,
            email: PostgresEmailVerificationRepository::new(db.clone()),
            reset: PostgresPasswordResetRepository::new(db),
        }
    }

    async fn find(&self, user: &UserId) -> Option<Found> {
        let now = Timestamp::now();
        match self.kind {
            CodeKind::Email => self.email.find_by_user(user).await.unwrap().map(|c| Found {
                id: *c.id().as_uuid(),
                attempts: c.attempts(),
                lockouts: c.lockouts(),
                sent: c.last_sent_at(),
                expires: c.expires_at(),
                expired: c.is_expired(now),
                reissue_allowed: c.reissue(now).is_allowed(),
            }),
            CodeKind::Reset => self.reset.find_by_user(user).await.unwrap().map(|c| Found {
                id: *c.id().as_uuid(),
                attempts: c.attempts(),
                lockouts: c.lockouts(),
                sent: c.last_sent_at(),
                expires: c.expires_at(),
                expired: c.is_expired(now),
                reissue_allowed: c.reissue(now).is_allowed(),
            }),
        }
    }

    /// Stores a code with the given state (the trace's initial code).
    async fn seed(
        &self,
        user: UserId,
        attempts: i32,
        sent: Timestamp,
        expires: Timestamp,
    ) -> (Uuid, IssueOutcome) {
        match self.kind {
            CodeKind::Email => {
                let code = EmailVerificationCode::from_persistence(
                    VerificationCodeId::new(),
                    user,
                    "hash".to_string(),
                    expires,
                    attempts,
                    sent,
                    sent,
                    None,
                    None,
                );
                (*code.id().as_uuid(), self.email.save(&code).await.unwrap())
            }
            CodeKind::Reset => {
                let code = PasswordResetCode::from_persistence(
                    ResetCodeId::new(),
                    user,
                    "hash".to_string(),
                    expires,
                    attempts,
                    sent,
                    sent,
                );
                (*code.id().as_uuid(), self.reset.save(&code).await.unwrap())
            }
        }
    }

    /// Issues a new code the way the use cases do: built by the production
    /// constructor (send time and expiry stamped there), then saved.
    async fn issue(&self, user: UserId) -> (Uuid, IssueOutcome) {
        match self.kind {
            CodeKind::Email => {
                let code = EmailVerificationCode::new(user, "hash".to_string());
                (*code.id().as_uuid(), self.email.save(&code).await.unwrap())
            }
            CodeKind::Reset => {
                let code = PasswordResetCode::new(user, "hash".to_string());
                (*code.id().as_uuid(), self.reset.save(&code).await.unwrap())
            }
        }
    }

    /// Registration's release of the cooldown after a failed email.
    async fn release_cooldown(&self, user: &UserId, id: Uuid) {
        let released = Timestamp::now() - SignedDuration::from_mins(RESEND_COOLDOWN_MINUTES);
        match self.kind {
            CodeKind::Email => self
                .email
                .set_last_sent_at(user, &VerificationCodeId::from_uuid(id), released)
                .await
                .unwrap(),
            CodeKind::Reset => panic!("password reset has no cooldown release"),
        }
    }

    async fn claim(&self, user: &UserId, id: Uuid) -> AttemptOutcome {
        let cap = domain::entities::attempt_lockout::MAX_ATTEMPTS;
        match self.kind {
            CodeKind::Email => self
                .email
                .register_attempt(user, &VerificationCodeId::from_uuid(id), cap)
                .await
                .unwrap(),
            CodeKind::Reset => self
                .reset
                .register_attempt(user, &ResetCodeId::from_uuid(id), cap)
                .await
                .unwrap(),
        }
    }

    async fn release(&self, user: &UserId, id: Uuid) {
        match self.kind {
            CodeKind::Email => self
                .email
                .release_attempt(user, &VerificationCodeId::from_uuid(id))
                .await
                .unwrap(),
            CodeKind::Reset => self
                .reset
                .release_attempt(user, &ResetCodeId::from_uuid(id))
                .await
                .unwrap(),
        }
    }

    /// What a correct guess does to the code: verify-email deletes the user's
    /// code; a password reset consumes the code it validated, by id.
    async fn consume(&self, user: &UserId, id: Uuid) {
        match self.kind {
            CodeKind::Email => self.email.delete_for_user(user).await.unwrap(),
            CodeKind::Reset => {
                self.reset
                    .consume(user, &ResetCodeId::from_uuid(id))
                    .await
                    .unwrap();
            }
        }
    }

    async fn cleanup(&self) {
        match self.kind {
            CodeKind::Email => self.email.delete_expired().await.unwrap(),
            CodeKind::Reset => self.reset.delete_expired().await.unwrap(),
        };
    }
}

/// How often each interesting outcome came up, so the suite can't pass on
/// traces that never reach the races it is for.
#[derive(Debug, Default)]
struct CodeCoverage {
    issue_refused: usize,
    issue_withheld: usize,
    claim_locked: usize,
    claim_not_found: usize,
    consumed: usize,
    cleaned_up: usize,
    cooldown_released: usize,
}

struct CodeReplay<'a> {
    kind: CodeKind,
    pool: &'a PgPool,
    observer: Codes,
    guesses: BTreeMap<i64, Codes>,
    issues: BTreeMap<i64, Codes>,
    cleaner: Codes,
}

impl CodeReplay<'_> {
    async fn check(
        &self,
        user: &UserId,
        ids: &BTreeMap<i64, Uuid>,
        state: &CodeState,
        at_step: &str,
    ) {
        let stored = self.observer.find(user).await;
        let row = &state.row;
        assert_eq!(
            stored.is_some(),
            row.present,
            "{at_step}: code row present in the database vs the model"
        );
        let Some(stored) = stored else { return };
        assert_eq!(
            Some(&stored.id),
            ids.get(&row.id),
            "{at_step}: which code is stored"
        );
        assert_eq!(
            i64::from(stored.attempts),
            row.attempts,
            "{at_step}: attempts"
        );
        assert_eq!(
            i64::from(stored.lockouts),
            row.lockouts,
            "{at_step}: lockouts"
        );
        assert_near(
            &format!("{at_step}: last_sent_at"),
            stored.sent,
            at(state.now, row.sent),
        );
        assert_near(
            &format!("{at_step}: expires_at"),
            stored.expires,
            at(state.now, row.expires),
        );
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one match arm per model step reads best as one function"
    )]
    async fn replay(&self, path: &Path, n: usize, coverage: &mut CodeCoverage) {
        let states: Vec<CodeState> = load(path);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        sqlx::query(self.kind.clear())
            .execute(self.pool)
            .await
            .unwrap();
        let user = new_user(self.pool, &format!("{name}-{n}").replace('.', "-")).await;

        // Model code id -> real id, and each request's in-flight read.
        let mut ids: BTreeMap<i64, Uuid> = BTreeMap::new();
        let mut read: BTreeMap<i64, Uuid> = BTreeMap::new();
        let mut issued: BTreeMap<i64, Uuid> = BTreeMap::new();

        let first = &states[0];
        if first.row.present {
            let (id, outcome) = self
                .observer
                .seed(
                    user,
                    i32::try_from(first.row.attempts).unwrap(),
                    at(first.now, first.row.sent),
                    at(first.now, first.row.expires),
                )
                .await;
            ids.insert(first.row.id, id);
            assert_eq!(outcome, IssueOutcome::Issued);
            sqlx::query(self.kind.set_lockouts())
                .bind(user.as_uuid())
                .bind(i32::try_from(first.row.lockouts).unwrap())
                .execute(self.pool)
                .await
                .unwrap();
        }
        self.check(&user, &ids, first, &format!("{name} initial state"))
            .await;

        for (index, pair) in states.windows(2).enumerate() {
            let (before, after) = (&pair[0], &pair[1]);
            let step = &after.last;
            let who = step.who;
            let at_step = format!("{name} step {} {}({who})", index + 1, step.act);
            match step.act.as_str() {
                "guessRead" => {
                    let live = self.guesses[&who].find(&user).await.filter(|c| !c.expired);
                    let claims = after.guess_pc[&who] == "claim";
                    assert_eq!(live.is_some(), claims, "{at_step}: a live code was read");
                    if let Some(code) = live {
                        assert_eq!(
                            Some(&code.id),
                            ids.get(&after.guess_code[&who]),
                            "{at_step}: code read"
                        );
                        read.insert(who, code.id);
                    }
                }
                "guessClaim" => {
                    let outcome = self.guesses[&who].claim(&user, read[&who]).await;
                    let expected = if after.guess_pc[&who] == "compare" {
                        AttemptOutcome::Claimed(i32::try_from(after.row.attempts).unwrap())
                    } else if after.guess_out[&who] == "locked" {
                        coverage.claim_locked += 1;
                        AttemptOutcome::Locked
                    } else {
                        coverage.claim_not_found += 1;
                        AttemptOutcome::NotFound
                    };
                    assert_eq!(outcome, expected, "{at_step}: register_attempt");
                }
                // The Argon2 comparison touches no shared state.
                "guessCompare" => {}
                "guessRelease" => self.guesses[&who].release(&user, read[&who]).await,
                "guessConsume" => {
                    self.guesses[&who].consume(&user, read[&who]).await;
                    if before.row.present && !after.row.present {
                        coverage.consumed += 1;
                    }
                }
                "issueDecide" => {
                    let allowed = self.issues[&who]
                        .find(&user)
                        .await
                        .is_none_or(|code| code.reissue_allowed);
                    assert_eq!(
                        allowed,
                        after.issue_pc[&who] == "write",
                        "{at_step}: reissue decision"
                    );
                    if !allowed {
                        coverage.issue_withheld += 1;
                    }
                }
                "issueWrite" => {
                    let (id, outcome) = self.issues[&who].issue(user).await;
                    if after.issue_out[&who] == "issued" {
                        assert_eq!(outcome, IssueOutcome::Issued, "{at_step}: issuing upsert");
                        assert_eq!(
                            after.row.expires - after.row.sent,
                            self.kind.expiry_minutes(),
                            "{at_step}: the model's code lifetime matches the domain's"
                        );
                        ids.insert(after.issue_code[&who], id);
                        issued.insert(who, id);
                    } else {
                        assert_eq!(outcome, IssueOutcome::Withheld, "{at_step}: issuing upsert");
                        coverage.issue_refused += 1;
                    }
                }
                "issueDeliver" => {
                    if step.flag && matches!(self.kind, CodeKind::Email) {
                        self.issues[&who]
                            .release_cooldown(&user, issued[&who])
                            .await;
                        coverage.cooldown_released += 1;
                    }
                }
                "cleanup" => {
                    self.cleaner.cleanup().await;
                    if before.row.present && !after.row.present {
                        coverage.cleaned_up += 1;
                    }
                }
                "tick" => {
                    let minutes = i32::try_from(after.now - before.now).unwrap();
                    sqlx::query(self.kind.rewind())
                        .bind(minutes)
                        .execute(self.pool)
                        .await
                        .unwrap();
                }
                other => panic!("{at_step}: unknown model step {other}"),
            }
            self.check(&user, &ids, after, &at_step).await;
        }
    }
}

async fn replay_code_traces(pool: PgPool, kind: CodeKind, prefix: &str) {
    let mut guesses = BTreeMap::new();
    for who in 1..=3 {
        guesses.insert(who, Codes::new(kind, &pool).await);
    }
    let mut issues = BTreeMap::new();
    for who in 1..=2 {
        issues.insert(who, Codes::new(kind, &pool).await);
    }
    let replay = CodeReplay {
        kind,
        pool: &pool,
        observer: Codes::new(kind, &pool).await,
        guesses,
        issues,
        cleaner: Codes::new(kind, &pool).await,
    };
    let mut coverage = CodeCoverage::default();
    let files = traces(prefix);
    for (n, path) in files.iter().enumerate() {
        replay.replay(path, n, &mut coverage).await;
    }
    eprintln!("{prefix}: {} traces replayed, {coverage:?}", files.len());
    assert!(
        coverage.issue_refused > 0,
        "no trace refused an issue: {coverage:?}"
    );
    assert!(
        coverage.issue_withheld > 0,
        "no trace withheld an issue: {coverage:?}"
    );
    assert!(
        coverage.claim_locked > 0,
        "no trace hit a locked claim: {coverage:?}"
    );
    assert!(
        coverage.claim_not_found > 0,
        "no trace hit a gone code: {coverage:?}"
    );
    assert!(
        coverage.consumed > 0,
        "no trace consumed a code: {coverage:?}"
    );
    assert!(
        coverage.cleaned_up > 0,
        "no trace cleaned up a code: {coverage:?}"
    );
    if matches!(kind, CodeKind::Email) {
        assert!(
            coverage.cooldown_released > 0,
            "no trace released a cooldown: {coverage:?}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "replays exported model traces; run with `just models`"]
async fn email_codes_follow_the_attempt_claim_model(pool: PgPool) {
    replay_code_traces(pool, CodeKind::Email, "email_codes").await;
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "replays exported model traces; run with `just models`"]
async fn reset_codes_follow_the_attempt_claim_model(pool: PgPool) {
    replay_code_traces(pool, CodeKind::Reset, "reset_codes").await;
}

// ── refresh_rotation.qnt ───────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct Token {
    exists: bool,
    revoked: bool,
    expires: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct TokenStep {
    act: String,
    who: i64,
    token: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenState {
    tokens: BTreeMap<i64, Token>,
    now: i64,
    next_id: i64,
    refresh_pc: BTreeMap<i64, String>,
    refresh_out: BTreeMap<i64, String>,
    logout_pc: BTreeMap<i64, String>,
    last: TokenStep,
}

/// The refresh token's lifetime in the model (30 days), in minutes.
const TOKEN_TTL_MINUTES: i64 = 30 * 24 * 60;

const FINGERPRINT: &str = "model-device";

fn token_hash(trace: usize, token: i64) -> String {
    format!("trace-{trace}-token-{token}")
}

#[derive(Debug, Default)]
struct TokenCoverage {
    rotated: usize,
    conflict: usize,
    reuse: usize,
    unknown: usize,
    logged_out: usize,
}

struct TokenReplay<'a> {
    pool: &'a PgPool,
    observer: PostgresRefreshTokenRepository,
    refreshes: BTreeMap<i64, PostgresRefreshTokenRepository>,
    logouts: BTreeMap<i64, PostgresRefreshTokenRepository>,
    cleaner: PostgresRefreshTokenRepository,
}

impl TokenReplay<'_> {
    async fn check(&self, n: usize, state: &TokenState, at_step: &str) {
        for (&id, model) in state.tokens.iter().filter(|(id, _)| **id < state.next_id) {
            let stored = self
                .observer
                .find_by_hash(&token_hash(n, id))
                .await
                .unwrap();
            assert_eq!(
                stored.is_some(),
                model.exists,
                "{at_step}: token {id} stored"
            );
            let Some(stored) = stored else { continue };
            assert_eq!(
                stored.is_revoked(),
                model.revoked,
                "{at_step}: token {id} revoked"
            );
            assert_near(
                &format!("{at_step}: token {id} expires_at"),
                stored.expires_at(),
                at(state.now, model.expires),
            );
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one match arm per model step reads best as one function"
    )]
    async fn replay(&self, path: &Path, n: usize, coverage: &mut TokenCoverage) {
        let states: Vec<TokenState> = load(path);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        sqlx::query("DELETE FROM refresh_tokens")
            .execute(self.pool)
            .await
            .unwrap();
        let user = new_user(self.pool, &format!("{name}-{n}").replace('.', "-")).await;

        // Each request's in-flight read. Tokens are found by their hash,
        // which encodes the model's token id.
        let mut read: BTreeMap<(&str, i64), RefreshTokenId> = BTreeMap::new();

        let first = &states[0];
        let initial = RefreshToken::new(
            user,
            token_hash(n, 0),
            FINGERPRINT.to_string(),
            at(first.now, first.tokens[&0].expires),
        );
        self.observer.save(&initial).await.unwrap();
        self.check(n, first, &format!("{name} initial state")).await;

        for (index, pair) in states.windows(2).enumerate() {
            let (before, after) = (&pair[0], &pair[1]);
            let step = &after.last;
            let who = step.who;
            let at_step = format!(
                "{name} step {} {}({who}, token {})",
                index + 1,
                step.act,
                step.token
            );
            match step.act.as_str() {
                "refreshRead" => {
                    let found = self.refreshes[&who]
                        .find_by_hash(&token_hash(n, step.token))
                        .await
                        .unwrap();
                    let (pc, out) = (&after.refresh_pc[&who], &after.refresh_out[&who]);
                    match found
                        .as_ref()
                        .map(|token| token.status(jiff::Timestamp::now()))
                    {
                        None => {
                            assert_eq!(out, "invalid", "{at_step}: unknown token");
                            coverage.unknown += 1;
                        }
                        Some(RefreshTokenStatus::Revoked) => {
                            assert_eq!(pc, "revokeAll", "{at_step}: revoked token");
                        }
                        Some(RefreshTokenStatus::Expired) => {
                            assert_eq!(out, "expired", "{at_step}: expired token");
                        }
                        Some(RefreshTokenStatus::Active) => {
                            assert_eq!(pc, "rotate", "{at_step}: active token");
                        }
                    }
                    if let Some(token) = found {
                        read.insert(("refresh", who), *token.id());
                    }
                }
                "refreshRotate" => {
                    let successor = RefreshToken::new(
                        user,
                        token_hash(n, before.next_id),
                        FINGERPRINT.to_string(),
                        Timestamp::now() + SignedDuration::from_mins(TOKEN_TTL_MINUTES),
                    );
                    let result = self.refreshes[&who]
                        .rotate(&read[&("refresh", who)], &successor)
                        .await;
                    if after.refresh_out[&who] == "rotated" {
                        assert!(result.is_ok(), "{at_step}: rotate failed: {result:?}");
                        coverage.rotated += 1;
                    } else {
                        assert!(
                            matches!(result, Err(DomainError::Conflict(_))),
                            "{at_step}: rotate should conflict, got {result:?}"
                        );
                        coverage.conflict += 1;
                    }
                }
                "refreshRevokeAll" => {
                    self.refreshes[&who]
                        .revoke_all_for_user(&user)
                        .await
                        .unwrap();
                    coverage.reuse += 1;
                }
                "logoutRead" => {
                    let found = self.logouts[&who]
                        .find_by_hash(&token_hash(n, step.token))
                        .await
                        .unwrap();
                    let revokes = found.as_ref().is_some_and(|t| !t.is_revoked());
                    assert_eq!(
                        revokes,
                        after.logout_pc[&who] == "revoke",
                        "{at_step}: logout read"
                    );
                    if let Some(token) = found {
                        read.insert(("logout", who), *token.id());
                    }
                }
                "logoutRevoke" => {
                    self.logouts[&who]
                        .revoke(&read[&("logout", who)])
                        .await
                        .unwrap();
                    coverage.logged_out += 1;
                }
                "cleanup" => {
                    self.cleaner.delete_expired().await.unwrap();
                }
                "tick" => {
                    let minutes = i32::try_from(after.now - before.now).unwrap();
                    sqlx::query(
                        "UPDATE refresh_tokens SET \
                         expires_at = expires_at - make_interval(mins => $1), \
                         created_at = created_at - make_interval(mins => $1), \
                         revoked_at = revoked_at - make_interval(mins => $1)",
                    )
                    .bind(minutes)
                    .execute(self.pool)
                    .await
                    .unwrap();
                }
                other => panic!("{at_step}: unknown model step {other}"),
            }
            self.check(n, after, &at_step).await;
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "replays exported model traces; run with `just models`"]
async fn refresh_tokens_follow_the_rotation_model(pool: PgPool) {
    let mut refreshes = BTreeMap::new();
    for who in 1..=3 {
        refreshes.insert(
            who,
            PostgresRefreshTokenRepository::new(session(&pool).await),
        );
    }
    let replay = TokenReplay {
        pool: &pool,
        observer: PostgresRefreshTokenRepository::new(session(&pool).await),
        refreshes,
        logouts: BTreeMap::from([(1, PostgresRefreshTokenRepository::new(session(&pool).await))]),
        cleaner: PostgresRefreshTokenRepository::new(session(&pool).await),
    };
    let mut coverage = TokenCoverage::default();
    let files = traces("refresh_rotation");
    for (n, path) in files.iter().enumerate() {
        replay.replay(path, n, &mut coverage).await;
    }
    eprintln!(
        "refresh_rotation: {} traces replayed, {coverage:?}",
        files.len()
    );
    assert!(
        coverage.rotated > 0,
        "no trace rotated a token: {coverage:?}"
    );
    assert!(
        coverage.conflict > 0,
        "no trace hit a rotation conflict: {coverage:?}"
    );
    assert!(coverage.reuse > 0, "no trace detected reuse: {coverage:?}");
    assert!(
        coverage.unknown > 0,
        "no trace presented a deleted token: {coverage:?}"
    );
    assert!(coverage.logged_out > 0, "no trace logged out: {coverage:?}");
}
