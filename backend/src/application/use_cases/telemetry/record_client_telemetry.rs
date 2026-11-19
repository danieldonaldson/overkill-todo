//! Record browser reports (OT-006 Decision 2): the rules between the validated
//! request and the sink.
//!
//! 1. Map `page` onto the site's page templates, `island` onto the islands,
//!    and `release` onto this replica's release labels, each with `other`.
//! 2. Derive the browser family and device class from the `User-Agent`.
//! 3. Re-scrub every text field ([`super::scrub`]).
//! 4. Fingerprint each error for grouping.
//! 5. Keep within the recording budget; count, but don't record, the rest.
//! 6. Hand the result to the [`ClientReportSink`].

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use super::scrub::scrub;
use crate::application::dto::client_telemetry_dto::{MAX_MESSAGE_CHARS, MAX_STACK_CHARS};
use crate::application::dto::{ClientErrorEvent, ClientEvent, ClientReportBatch};
use crate::application::ports::incoming::RecordClientTelemetry;
use crate::application::ports::outgoing::ClientPageLoadReport;
use crate::application::ports::outgoing::{ClientErrorReport, ClientReportSink};
use crate::domain::value_objects::UserId;
use crate::shared::fingerprint::generate_error_fingerprint;

/// Default for `CLIENT_TELEMETRY_MAX_EVENTS_PER_SEC` (Decision 9).
pub const DEFAULT_MAX_EVENTS_PER_SEC: u32 = 200;

/// Longest `source` and `api_route` kept, in characters (the DTO's limits).
const MAX_SOURCE_CHARS: usize = 500;
const MAX_API_ROUTE_CHARS: usize = 200;

/// The `source` the browser sends for an error from another origin.
const EXTERNAL_SOURCE: &str = "external";

/// The site's page templates (Decision 5): every `apps/web/src/pages/**/*.astro`
/// route. A test checks this list against that directory.
pub const PAGE_TEMPLATES: &[&str] = &[
    "/login",
    "/register",
    "/forgot-password",
    "/reset-password",
    "/verify-email",
    "/verify-magic",
    "/set-password",
    "/dashboard",
    "/dashboard/settings",
    "/dashboard/admin",
];

/// The site's islands (Decision 6b): the `name` of every `TelemetryBoundary`.
///
/// A test checks this list against `tests/fixtures/client_islands.json`, which
/// the web guard test checks against the islands themselves.
pub const ISLAND_TEMPLATES: &[&str] = &[
    "AdminPanel",
    "AuthScreen",
    "DashboardHeader",
    "ForgotPasswordForm",
    "HomePanel",
    "ModeWatcher",
    "ResetPasswordForm",
    "SetPasswordForm",
    "SettingsPanel",
    "VerifyEmailForm",
    "VerifyMagicForm",
];

/// Distinct releases one replica labels in metrics; later ones are `other`.
pub const MAX_RELEASE_LABELS: usize = 5;

/// Page label for anything not in [`PAGE_TEMPLATES`].
pub const OTHER_PAGE: &str = "other";

/// The page template for a `location.pathname`, or [`OTHER_PAGE`].
#[must_use]
pub fn page_template(path: &str) -> &'static str {
    let path = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    };
    PAGE_TEMPLATES
        .iter()
        .find(|template| **template == path)
        .copied()
        .unwrap_or(OTHER_PAGE)
}

/// The island name from [`ISLAND_TEMPLATES`], or [`OTHER_PAGE`] (`other`).
#[must_use]
pub fn island_template(name: &str) -> &'static str {
    ISLAND_TEMPLATES
        .iter()
        .find(|island| **island == name)
        .copied()
        .unwrap_or(OTHER_PAGE)
}

/// The releases this replica uses as metric labels (Decision 7): its own
/// release, then the first ones reports name, up to [`MAX_RELEASE_LABELS`].
/// The client chooses `release`, so without a cap it could mint series.
#[derive(Debug)]
struct ReleaseLabels(Mutex<Vec<String>>);

impl ReleaseLabels {
    fn new(own: Option<&str>) -> Self {
        Self(Mutex::new(own.map(str::to_string).into_iter().collect()))
    }

    fn label(&self, release: &str) -> String {
        let mut labels = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if labels.iter().any(|l| l == release) {
            return release.to_string();
        }
        if labels.len() < MAX_RELEASE_LABELS {
            labels.push(release.to_string());
            return release.to_string();
        }
        drop(labels);
        OTHER_PAGE.to_string()
    }
}

/// `mobile`, `tablet` or `desktop`, from substrings every major browser puts in
/// its user agent. Good enough for a metric label; no UA database.
#[must_use]
pub fn device_class(user_agent: &str) -> &'static str {
    if user_agent.contains("iPad") || user_agent.contains("Tablet") {
        "tablet"
    } else if user_agent.contains("Mobi") {
        "mobile"
    } else if user_agent.contains("Android") {
        // Android without "Mobile" is a tablet by Google's own convention.
        "tablet"
    } else {
        "desktop"
    }
}

/// The browser family, from its user-agent token. Order matters: Edge, Opera
/// and Samsung Internet also claim to be Chrome, and Chrome claims Safari.
#[must_use]
pub fn browser_family(user_agent: &str) -> &'static str {
    const FAMILIES: &[(&[&str], &str)] = &[
        (&["Edg/", "EdgA/", "EdgiOS/"], "edge"),
        (&["OPR/", "OPT/"], "opera"),
        (&["SamsungBrowser/"], "samsung"),
        (&["Firefox/", "FxiOS/"], "firefox"),
        (&["Chrome/", "CriOS/", "Chromium/"], "chrome"),
        (&["Safari/"], "safari"),
    ];
    FAMILIES
        .iter()
        .find(|(tokens, _)| tokens.iter().any(|t| user_agent.contains(t)))
        .map_or("other", |(_, family)| family)
}

/// The first stack line that names a frame (`at …` in Chrome, `fn@url` in
/// Firefox and Safari), or `""`.
fn top_frame(stack: &str) -> &str {
    stack
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("at ") || line.contains('@'))
        .unwrap_or("")
}

/// A token bucket: `rate` tokens a second, holding at most `rate`.
#[derive(Debug)]
struct Budget {
    rate: f64,
    state: Mutex<BudgetState>,
}

#[derive(Debug)]
struct BudgetState {
    tokens: f64,
    refilled_at: Instant,
}

impl Budget {
    fn new(rate: u32, now: Instant) -> Self {
        let rate = f64::from(rate);
        Self {
            rate,
            state: Mutex::new(BudgetState {
                tokens: rate,
                refilled_at: now,
            }),
        }
    }

    /// Take up to `wanted` tokens at `now`; returns how many were granted.
    fn take(&self, wanted: usize, now: Instant) -> usize {
        // A panic while holding the lock can't leave the numbers inconsistent.
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let elapsed = now
            .saturating_duration_since(state.refilled_at)
            .as_secs_f64();
        state.tokens = elapsed.mul_add(self.rate, state.tokens).min(self.rate);
        state.refilled_at = now;
        let mut granted = 0;
        while granted < wanted && state.tokens >= 1.0 {
            state.tokens -= 1.0;
            granted += 1;
        }
        drop(state);
        granted
    }
}

/// Records browser reports within a global budget per replica.
pub struct RecordClientTelemetryUseCase {
    sink: Arc<dyn ClientReportSink>,
    budget: Budget,
    releases: ReleaseLabels,
}

impl RecordClientTelemetryUseCase {
    /// `max_events_per_sec` is `CLIENT_TELEMETRY_MAX_EVENTS_PER_SEC`.
    #[must_use]
    pub fn new(sink: Arc<dyn ClientReportSink>, max_events_per_sec: u32) -> Self {
        Self {
            sink,
            budget: Budget::new(max_events_per_sec, Instant::now()),
            releases: ReleaseLabels::new(None),
        }
    }

    /// Reserve a release label for `release`, this replica's own build
    /// (`APP_VERSION`), so reports from it are always labelled however many
    /// other releases arrive first.
    #[must_use]
    pub fn with_own_release(mut self, release: &str) -> Self {
        self.releases = ReleaseLabels::new(Some(release));
        self
    }

    fn record_at(
        &self,
        batch: ClientReportBatch,
        user: Option<UserId>,
        user_agent: &str,
        now: Instant,
    ) {
        let granted = self.budget.take(batch.events.len(), now);
        let dropped = batch.events.len() - granted;

        let page = page_template(&batch.page);
        let browser_family = browser_family(user_agent);
        let device_class = device_class(user_agent);
        let mut release_label = None;

        for event in batch.events.into_iter().take(granted) {
            match event {
                ClientEvent::Error(error) => {
                    let release_label = release_label
                        .get_or_insert_with(|| self.releases.label(&batch.release))
                        .as_str();
                    let report = error_report(
                        error,
                        ReportContext {
                            page,
                            release: &batch.release,
                            release_label,
                            page_load_id: &batch.page_load_id,
                            browser_family,
                            device_class,
                            user_id: user,
                        },
                    );
                    self.sink.error(&report);
                }
                ClientEvent::PageLoad => self.sink.page_load(&ClientPageLoadReport {
                    page,
                    device_class,
                    authenticated: user.is_some(),
                }),
            }
        }
        if dropped > 0 {
            self.sink.dropped(dropped as u64);
        }
    }
}

/// What every event in a batch shares.
#[derive(Clone, Copy)]
struct ReportContext<'a> {
    page: &'static str,
    release: &'a str,
    release_label: &'a str,
    page_load_id: &'a str,
    browser_family: &'static str,
    device_class: &'static str,
    user_id: Option<UserId>,
}

fn error_report(error: ClientErrorEvent, cx: ReportContext<'_>) -> ClientErrorReport {
    let message = scrub(&error.message, MAX_MESSAGE_CHARS);
    let stack = error.stack.map(|s| scrub(&s, MAX_STACK_CHARS));
    let fingerprint_source = format!("{message}\n{}", stack.as_deref().map_or("", top_frame));
    ClientErrorReport {
        page: cx.page,
        release: cx.release.to_string(),
        release_label: cx.release_label.to_string(),
        kind: error.kind,
        fingerprint: generate_error_fingerprint(
            &format!("client:{}", error.kind.as_str()),
            &fingerprint_source,
        ),
        count: error.count,
        message,
        stack,
        external: error.source.as_deref() == Some(EXTERNAL_SOURCE),
        source: error.source.map(|s| scrub(&s, MAX_SOURCE_CHARS)),
        island: error.island.as_deref().map(island_template),
        api_route: error.api_route.map(|s| scrub(&s, MAX_API_ROUTE_CHARS)),
        status: error.status,
        network_failed: error.network_failed,
        client_trace_id: error.trace_id,
        page_load_id: cx.page_load_id.to_string(),
        browser_family: cx.browser_family,
        device_class: cx.device_class,
        user_id: cx.user_id,
    }
}

impl RecordClientTelemetry for RecordClientTelemetryUseCase {
    // Not instrumented: browser reports get no trace (the request has no span,
    // so a span here would start an exported trace of its own).
    fn record(&self, batch: ClientReportBatch, user: Option<UserId>, user_agent: &str) {
        self.record_at(batch, user, user_agent, Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use super::*;
    use crate::application::dto::ClientErrorKind;

    const CHROME_DESKTOP: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
    const SAFARI_IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";

    #[derive(Default)]
    struct RecordingSink {
        errors: Mutex<Vec<ClientErrorReport>>,
        page_loads: Mutex<Vec<ClientPageLoadReport>>,
        dropped: Mutex<Vec<u64>>,
    }

    impl ClientReportSink for RecordingSink {
        fn error(&self, report: &ClientErrorReport) {
            self.errors.lock().unwrap().push(report.clone());
        }
        fn page_load(&self, report: &ClientPageLoadReport) {
            self.page_loads.lock().unwrap().push(*report);
        }
        fn dropped(&self, n: u64) {
            self.dropped.lock().unwrap().push(n);
        }
    }

    impl RecordingSink {
        fn errors(&self) -> Vec<ClientErrorReport> {
            self.errors.lock().unwrap().clone()
        }
        fn dropped_total(&self) -> u64 {
            self.dropped.lock().unwrap().iter().sum()
        }
    }

    fn use_case(rate: u32) -> (RecordClientTelemetryUseCase, Arc<RecordingSink>) {
        let sink = Arc::new(RecordingSink::default());
        (RecordClientTelemetryUseCase::new(sink.clone(), rate), sink)
    }

    fn event(message: &str) -> ClientEvent {
        ClientEvent::Error(ClientErrorEvent {
            kind: ClientErrorKind::Error,
            message: message.to_string(),
            stack: Some(format!(
                "Error: {message}\n    at render (https://overkill.example/_astro/index.js:1:2)"
            )),
            source: None,
            island: None,
            api_route: None,
            status: None,
            network_failed: None,
            trace_id: None,
            count: 1,
        })
    }

    fn batch(page: &str, events: Vec<ClientEvent>) -> ClientReportBatch {
        ClientReportBatch {
            page_load_id: "0123456789abcdef0123456789abcdef".into(),
            release: "abc123".into(),
            page: page.into(),
            events,
        }
    }

    fn record_one(page: &str, event: ClientEvent, user: Option<UserId>) -> ClientErrorReport {
        let (uc, sink) = use_case(DEFAULT_MAX_EVENTS_PER_SEC);
        uc.record(batch(page, vec![event]), user, CHROME_DESKTOP);
        let errors = sink.errors();
        assert_eq!(errors.len(), 1);
        errors.into_iter().next().unwrap()
    }

    #[test]
    fn known_pages_keep_their_template_and_others_become_other() {
        assert_eq!(page_template("/login"), "/login");
        assert_eq!(page_template("/dashboard/settings/"), "/dashboard/settings");
        assert_eq!(page_template("/dashboard"), "/dashboard");
        assert_eq!(page_template("/"), OTHER_PAGE);
        assert_eq!(page_template("/wp-admin"), OTHER_PAGE);
        assert_eq!(page_template("/login?email=a@b.io"), OTHER_PAGE);
        assert_eq!(page_template(""), OTHER_PAGE);
        assert_eq!(record_one("/nope", event("x"), None).page, OTHER_PAGE);
    }

    /// Decision 5: the allowlist is exactly the site's `.astro` pages.
    #[test]
    fn page_templates_match_the_sites_pages() {
        fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut BTreeSet<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else if path.extension().is_some_and(|e| e == "astro") {
                    let rel = path.strip_prefix(root).unwrap().with_extension("");
                    let route = format!("/{}", rel.to_string_lossy());
                    let route = route.strip_suffix("/index").unwrap_or(&route);
                    out.insert(if route == "/index" {
                        "/".into()
                    } else {
                        route.into()
                    });
                }
            }
        }
        let pages = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../apps/web/src/pages");
        let mut routes = BTreeSet::new();
        walk(&pages, &pages, &mut routes);
        let listed: BTreeSet<String> = PAGE_TEMPLATES.iter().map(|p| (*p).to_string()).collect();
        assert_eq!(
            listed, routes,
            "update PAGE_TEMPLATES to match apps/web/src/pages"
        );
    }

    #[test]
    fn device_class_and_browser_family_come_from_the_user_agent() {
        let cases = [
            (CHROME_DESKTOP, "chrome", "desktop"),
            (SAFARI_IPHONE, "safari", "mobile"),
            (
                "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
                "safari",
                "tablet",
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36",
                "chrome",
                "mobile",
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/26.0 Chrome/122.0.0.0 Safari/537.36",
                "samsung",
                "tablet",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0",
                "edge",
                "desktop",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:133.0) Gecko/20100101 Firefox/133.0",
                "firefox",
                "desktop",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 OPR/115.0.0.0",
                "opera",
                "desktop",
            ),
            ("", "other", "desktop"),
            ("curl/8.11.0", "other", "desktop"),
        ];
        for (ua, family, class) in cases {
            assert_eq!(browser_family(ua), family, "{ua}");
            assert_eq!(device_class(ua), class, "{ua}");
        }
    }

    #[test]
    fn the_report_carries_the_batch_context_and_the_session_user() {
        let user = UserId::new();
        let (uc, sink) = use_case(DEFAULT_MAX_EVENTS_PER_SEC);
        uc.record(
            batch("/dashboard/settings", vec![event("boom")]),
            Some(user),
            SAFARI_IPHONE,
        );
        let report = sink.errors().pop().unwrap();
        assert_eq!(report.page, "/dashboard/settings");
        assert_eq!(report.release, "abc123");
        assert_eq!(report.page_load_id, "0123456789abcdef0123456789abcdef");
        assert_eq!(report.kind, ClientErrorKind::Error);
        assert_eq!(report.browser_family, "safari");
        assert_eq!(report.device_class, "mobile");
        assert_eq!(report.user_id, Some(user));
        assert_eq!(record_one("/login", event("x"), None).user_id, None);
    }

    #[test]
    fn every_text_field_is_scrubbed_again() {
        let report = record_one(
            "/reset-password",
            ClientEvent::Error(ClientErrorEvent {
                kind: ClientErrorKind::Api,
                message:
                    "failed for alice@example.com at /reset-password?email=alice%40example.com"
                        .into(),
                stack: Some(
                    "Error\n    at f (https://overkill.example/verify-magic#token=abc:1:2)".into(),
                ),
                source: Some("https://overkill.example/_astro/a.js?token=abc".into()),
                island: None,
                api_route: Some("/auth/reset?token=abc".into()),
                status: Some(500),
                network_failed: None,
                trace_id: Some("abcdef0123456789abcdef0123456789".into()),
                count: 3,
            }),
            None,
        );
        assert_eq!(report.message, "failed for [EMAIL] at /reset-password");
        assert_eq!(
            report.stack.as_deref(),
            Some("Error\n    at f (https://overkill.example/verify-magic:1:2)")
        );
        assert_eq!(
            report.source.as_deref(),
            Some("https://overkill.example/_astro/a.js")
        );
        assert_eq!(report.api_route.as_deref(), Some("/auth/reset"));
        assert_eq!(report.status, Some(500));
        assert_eq!(
            report.client_trace_id.as_deref(),
            Some("abcdef0123456789abcdef0123456789")
        );
        assert_eq!(report.count, 3);
    }

    #[test]
    fn text_is_truncated_after_scrubbing() {
        // Scrubbing can lengthen text (`a@b.io` → `[EMAIL]`); the cap still holds.
        let message = "a@b.io ".repeat(71); // 497 characters
        let report = record_one("/login", event(&message), None);
        assert_eq!(report.message.chars().count(), MAX_MESSAGE_CHARS);
    }

    #[test]
    fn fingerprints_group_the_same_error_and_split_different_ones() {
        let a = record_one("/login", event("x is undefined"), None).fingerprint;
        let b = record_one("/register", event("x is undefined"), None).fingerprint;
        let c = record_one("/login", event("y is undefined"), None).fingerprint;
        assert_eq!(a, b);
        assert_ne!(a, c);
        // The same message from another frame is another error.
        let mut other_frame = event("x is undefined");
        let ClientEvent::Error(e) = &mut other_frame else {
            unreachable!()
        };
        e.stack = Some("Error\n    at other (https://overkill.example/_astro/b.js:9:9)".into());
        assert_ne!(a, record_one("/login", other_frame, None).fingerprint);
        // Emails differ per user but the error is the same.
        assert_eq!(
            record_one("/login", event("no user a@b.io"), None).fingerprint,
            record_one("/login", event("no user c@d.io"), None).fingerprint
        );
    }

    #[test]
    fn top_frame_finds_chrome_and_firefox_frames() {
        assert_eq!(
            top_frame("TypeError: x\n    at f (a.js:1:2)\n    at g"),
            "at f (a.js:1:2)"
        );
        assert_eq!(
            top_frame("f@https://h/a.js:1:2\ng@b.js:3:4"),
            "f@https://h/a.js:1:2"
        );
        assert_eq!(top_frame("no frames here"), "");
    }

    #[test]
    fn events_over_the_budget_are_dropped_and_counted_never_recorded() {
        let (uc, sink) = use_case(3);
        let now = Instant::now();
        let events = (0..5).map(|i| event(&format!("e{i}"))).collect();
        uc.record_at(batch("/login", events), None, CHROME_DESKTOP, now);
        let recorded: Vec<String> = sink.errors().into_iter().map(|r| r.message).collect();
        assert_eq!(recorded, ["e0", "e1", "e2"]);
        assert_eq!(sink.dropped_total(), 2);

        // Still empty a moment later: nothing recorded, all counted.
        uc.record_at(
            batch("/login", vec![event("late")]),
            None,
            CHROME_DESKTOP,
            now + Duration::from_millis(100),
        );
        assert_eq!(sink.errors().len(), 3);
        assert_eq!(sink.dropped_total(), 3);
    }

    #[test]
    fn the_budget_refills_at_its_rate_and_never_beyond_it() {
        let start = Instant::now();
        let budget = Budget::new(10, start);
        assert_eq!(budget.take(10, start), 10);
        assert_eq!(budget.take(1, start), 0);
        assert_eq!(budget.take(10, start + Duration::from_millis(500)), 5);
        // A long quiet spell refills to the rate, not more.
        assert_eq!(budget.take(100, start + Duration::from_secs(60)), 10);
    }

    fn with_error(message: &str, edit: impl FnOnce(&mut ClientErrorEvent)) -> ClientEvent {
        let mut e = event(message);
        let ClientEvent::Error(inner) = &mut e else {
            unreachable!()
        };
        edit(inner);
        e
    }

    #[test]
    fn a_page_load_is_counted_with_the_page_device_and_session() {
        let (uc, sink) = use_case(DEFAULT_MAX_EVENTS_PER_SEC);
        uc.record(
            batch("/dashboard/settings/", vec![ClientEvent::PageLoad]),
            Some(UserId::new()),
            SAFARI_IPHONE,
        );
        uc.record(
            batch("/wp-admin", vec![ClientEvent::PageLoad]),
            None,
            CHROME_DESKTOP,
        );
        assert_eq!(
            *sink.page_loads.lock().unwrap(),
            [
                ClientPageLoadReport {
                    page: "/dashboard/settings",
                    device_class: "mobile",
                    authenticated: true,
                },
                ClientPageLoadReport {
                    page: OTHER_PAGE,
                    device_class: "desktop",
                    authenticated: false,
                },
            ]
        );
        assert_eq!(sink.errors(), Vec::new());
    }

    #[test]
    fn page_loads_share_the_budget() {
        let (uc, sink) = use_case(2);
        let events = vec![ClientEvent::PageLoad, event("a"), ClientEvent::PageLoad];
        uc.record_at(
            batch("/login", events),
            None,
            CHROME_DESKTOP,
            Instant::now(),
        );
        assert_eq!(sink.page_loads.lock().unwrap().len(), 1);
        assert_eq!(sink.errors().len(), 1);
        assert_eq!(sink.dropped_total(), 1);
    }

    #[test]
    fn known_islands_keep_their_name_and_others_become_other() {
        let island = |name: &str| {
            record_one(
                "/dashboard",
                with_error("x", |e| {
                    e.kind = ClientErrorKind::Boundary;
                    e.island = Some(name.into());
                }),
                None,
            )
            .island
        };
        assert_eq!(island("SettingsPanel"), Some("SettingsPanel"));
        assert_eq!(island("settingspanel"), Some(OTHER_PAGE));
        assert_eq!(island("<script>"), Some(OTHER_PAGE));
        assert_eq!(record_one("/login", event("x"), None).island, None);
    }

    /// Decision 6b: the allowlist is the shared fixture the web guard reads.
    #[test]
    fn island_templates_match_the_shared_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/client_islands.json"
        ))
        .unwrap();
        let listed: BTreeSet<&str> = fixture["islands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            listed,
            ISLAND_TEMPLATES.iter().copied().collect::<BTreeSet<_>>(),
            "update ISLAND_TEMPLATES to match tests/fixtures/client_islands.json"
        );
    }

    #[test]
    fn only_errors_from_another_origin_are_external() {
        let external = |source: Option<&str>| {
            record_one(
                "/login",
                with_error("x", |e| e.source = source.map(str::to_string)),
                None,
            )
            .external
        };
        assert!(external(Some("external")));
        assert!(!external(Some("https://overkill.example/_astro/a.js")));
        assert!(!external(None));
    }

    #[test]
    fn release_labels_are_capped_per_replica_but_logs_keep_the_release() {
        let sink = Arc::new(RecordingSink::default());
        let uc = RecordClientTelemetryUseCase::new(sink.clone(), DEFAULT_MAX_EVENTS_PER_SEC)
            .with_own_release("own");
        let send = |release: &str| {
            let mut b = batch("/login", vec![event("x")]);
            b.release = release.into();
            uc.record(b, None, CHROME_DESKTOP);
            let r = sink.errors().pop().unwrap();
            (r.release, r.release_label)
        };
        for release in ["r1", "r2", "r3", "r4"] {
            assert_eq!(send(release), (release.into(), release.into()));
        }
        // Own release plus four: the sixth distinct release is `other`.
        assert_eq!(send("r5"), ("r5".into(), OTHER_PAGE.into()));
        assert_eq!(send("own"), ("own".into(), "own".into()));
        assert_eq!(send("r1"), ("r1".into(), "r1".into()));
    }

    #[test]
    fn nothing_is_dropped_within_the_budget() {
        let (uc, sink) = use_case(DEFAULT_MAX_EVENTS_PER_SEC);
        let events = (0..30).map(|i| event(&format!("e{i}"))).collect();
        uc.record(batch("/login", events), None, CHROME_DESKTOP);
        assert_eq!(sink.errors().len(), 30);
        assert_eq!(sink.dropped_total(), 0);
        assert!(
            sink.dropped.lock().unwrap().is_empty(),
            "no dropped(0) calls"
        );
    }
}
