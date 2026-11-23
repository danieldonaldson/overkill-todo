// -*- mode: Bluespec; -*-
//
// Refresh-token rotation under concurrency (OT-010 design D8; threat model
// P-14, P-15). One user's refresh tokens, presented by concurrent refresh
// requests, signed out, and swept by the expiry cleanup job.
//
// One model step is one SQL statement, or one transaction (rotate runs its
// revoke and insert in one READ COMMITTED transaction; the revoke's
// `WHERE revoked_at IS NULL` re-checks the row after any concurrent writer
// commits, so the transaction is atomic with respect to these steps).
//
//   refresh  (POST /auth/refresh, RefreshTokenUseCase):
//     read       find_by_hash, then the domain's status(): unknown → "invalid",
//                revoked → reuse, expired → "expired", active → rotate
//     rotate     rotate(old, new): revoke old WHERE revoked_at IS NULL, insert
//                the successor; "conflict" if the revoke matched nothing
//     revokeAll  revoke_all_for_user, after a revoked token was presented
//   logout   (RevokeTokenUseCase): read (find_by_hash), then revoke(id)
//   cleanup  delete_expired
//   tick     time passes
//
// Time is whole minutes; as in attempt_claim.qnt, `x < NOW()` in SQL is
// `x <= now` here.
//
// Traces are replayed against PostgresRefreshTokenRepository by
// backend/crates/persistence-postgres/tests/model_conformance.rs (`just models`).
module refresh_rotation {
  // REFRESH_TOKEN_EXPIRATION_DAYS (30) in minutes.
  pure val TTL = 43200

  type Token = {
    exists: bool,
    revoked: bool,
    expires: int,
    // The token this one was rotated from, or -1.
    parent: int,
  }

  type Last = { act: str, who: int, token: int, delta: int }

  pure val REFRESHES = Set(1, 2, 3)
  pure val LOGOUTS = Set(1)
  // Token 0 is the session's first; each refresh mints at most one more.
  pure val TOKEN_IDS = 0.to(REFRESHES.size())

  var tokens: int -> Token
  var now: int
  var nextId: int

  var refreshPc: int -> str
  var refreshToken: int -> int
  var refreshOut: int -> str
  var logoutPc: int -> str
  var logoutToken: int -> int

  // Ghosts. Tokens that have been rotated (have a successor).
  var rotated: Set[int]
  // Each refresh read a token that had been rotated and was still stored.
  var sawRotated: int -> bool
  // Each refresh read a token that had been rotated, stored or not.
  var replayedRotated: int -> bool
  // A reuse has been detected and every session revoked.
  var reuseDetected: bool

  // The step just taken, for the conformance replay.
  var last: Last

  pure def did(act: str, who: int, token: int): Last =
    { act: act, who: who, token: token, delta: 0 }

  def live(t: int): bool =
    tokens.get(t).exists and not(tokens.get(t).revoked) and now < tokens.get(t).expires

  // ── Steps ───────────────────────────────────────────────────────────────

  action refreshRead(p: int, t: int): bool = {
    val tok = tokens.get(t)
    val nextPc =
      if (not(tok.exists)) "done"
      else if (tok.revoked) "revokeAll"
      else if (tok.expires <= now) "done"
      else "rotate"
    all {
      refreshPc.get(p) == "read",
      t < nextId,
      refreshPc' = refreshPc.set(p, nextPc),
      refreshToken' = refreshToken.set(p, t),
      refreshOut' = refreshOut.set(p,
        if (not(tok.exists)) "invalid"
        else if (nextPc == "done") "expired"
        else ""),
      sawRotated' = sawRotated.set(p, tok.exists and rotated.contains(t)),
      replayedRotated' = replayedRotated.set(p, rotated.contains(t)),
      tokens' = tokens, now' = now, nextId' = nextId,
      logoutPc' = logoutPc, logoutToken' = logoutToken,
      rotated' = rotated, reuseDetected' = reuseDetected,
      last' = did("refreshRead", p, t),
    }
  }

  action refreshRotate(p: int): bool = {
    val t = refreshToken.get(p)
    val tok = tokens.get(t)
    // The revoke matches only while the old token is stored and unrevoked.
    val won = tok.exists and not(tok.revoked)
    all {
      refreshPc.get(p) == "rotate",
      refreshPc' = refreshPc.set(p, "done"),
      if (won) all {
        tokens' = tokens
          .set(t, tok.with("revoked", true))
          .set(nextId, { exists: true, revoked: false, expires: now + TTL, parent: t }),
        nextId' = nextId + 1,
        rotated' = rotated.union(Set(t)),
        refreshOut' = refreshOut.set(p, "rotated"),
      } else all {
        tokens' = tokens,
        nextId' = nextId,
        rotated' = rotated,
        refreshOut' = refreshOut.set(p, "conflict"),
      },
      now' = now, refreshToken' = refreshToken,
      logoutPc' = logoutPc, logoutToken' = logoutToken,
      sawRotated' = sawRotated, replayedRotated' = replayedRotated,
      reuseDetected' = reuseDetected,
      last' = did("refreshRotate", p, t),
    }
  }

  action refreshRevokeAll(p: int): bool = all {
    refreshPc.get(p) == "revokeAll",
    refreshPc' = refreshPc.set(p, "done"),
    refreshOut' = refreshOut.set(p, "reuse"),
    tokens' = TOKEN_IDS.mapBy(t =>
      if (tokens.get(t).exists) tokens.get(t).with("revoked", true) else tokens.get(t)),
    reuseDetected' = true,
    now' = now, nextId' = nextId, refreshToken' = refreshToken,
    logoutPc' = logoutPc, logoutToken' = logoutToken,
    rotated' = rotated, sawRotated' = sawRotated, replayedRotated' = replayedRotated,
    last' = did("refreshRevokeAll", p, refreshToken.get(p)),
  }

  action logoutRead(l: int, t: int): bool = all {
    logoutPc.get(l) == "read",
    t < nextId,
    logoutPc' = logoutPc.set(l,
      if (tokens.get(t).exists and not(tokens.get(t).revoked)) "revoke" else "done"),
    logoutToken' = logoutToken.set(l, t),
    tokens' = tokens, now' = now, nextId' = nextId,
    refreshPc' = refreshPc, refreshToken' = refreshToken, refreshOut' = refreshOut,
    rotated' = rotated, sawRotated' = sawRotated, replayedRotated' = replayedRotated,
    reuseDetected' = reuseDetected,
    last' = did("logoutRead", l, t),
  }

  action logoutRevoke(l: int): bool = {
    val t = logoutToken.get(l)
    all {
      logoutPc.get(l) == "revoke",
      logoutPc' = logoutPc.set(l, "done"),
      tokens' = if (tokens.get(t).exists) tokens.set(t, tokens.get(t).with("revoked", true))
        else tokens,
      now' = now, nextId' = nextId, logoutToken' = logoutToken,
      refreshPc' = refreshPc, refreshToken' = refreshToken, refreshOut' = refreshOut,
      rotated' = rotated, sawRotated' = sawRotated, replayedRotated' = replayedRotated,
      reuseDetected' = reuseDetected,
      last' = did("logoutRevoke", l, t),
    }
  }

  action cleanup: bool = all {
    tokens' = TOKEN_IDS.mapBy(t =>
      if (tokens.get(t).exists and tokens.get(t).expires <= now)
        tokens.get(t).with("exists", false)
      else tokens.get(t)),
    now' = now, nextId' = nextId,
    refreshPc' = refreshPc, refreshToken' = refreshToken, refreshOut' = refreshOut,
    logoutPc' = logoutPc, logoutToken' = logoutToken,
    rotated' = rotated, sawRotated' = sawRotated, replayedRotated' = replayedRotated,
    reuseDetected' = reuseDetected,
    last' = did("cleanup", 0, -1),
  }

  action tick(delta: int): bool = all {
    now' = now + delta,
    tokens' = tokens, nextId' = nextId,
    refreshPc' = refreshPc, refreshToken' = refreshToken, refreshOut' = refreshOut,
    logoutPc' = logoutPc, logoutToken' = logoutToken,
    rotated' = rotated, sawRotated' = sawRotated, replayedRotated' = replayedRotated,
    reuseDetected' = reuseDetected,
    last' = { act: "tick", who: 0, token: -1, delta: delta },
  }

  action init = all {
    tokens' = TOKEN_IDS.mapBy(t =>
      { exists: t == 0, revoked: false, expires: if (t == 0) TTL else 0, parent: -1 }),
    now' = 0,
    nextId' = 1,
    refreshPc' = REFRESHES.mapBy(_ => "read"),
    refreshToken' = REFRESHES.mapBy(_ => -1),
    refreshOut' = REFRESHES.mapBy(_ => ""),
    logoutPc' = LOGOUTS.mapBy(_ => "read"),
    logoutToken' = LOGOUTS.mapBy(_ => -1),
    rotated' = Set(),
    sawRotated' = REFRESHES.mapBy(_ => false),
    replayedRotated' = REFRESHES.mapBy(_ => false),
    reuseDetected' = false,
    last' = did("init", 0, -1),
  }

  action step = any {
    nondet p = REFRESHES.oneOf()
    nondet t = TOKEN_IDS.oneOf()
    any { refreshRead(p, t), refreshRotate(p), refreshRevokeAll(p) },
    nondet l = LOGOUTS.oneOf()
    nondet u = TOKEN_IDS.oneOf()
    any { logoutRead(l, u), logoutRevoke(l) },
    cleanup,
    nondet delta = Set(1, 1440, 43200).oneOf()
    tick(delta),
  }

  // ── Invariants ──────────────────────────────────────────────────────────

  // P-14: however many requests present one token at once, at most one gets
  // a successor.
  val atMostOneSuccessorPerToken =
    TOKEN_IDS.forall(t => TOKEN_IDS.filter(s => tokens.get(s).parent == t).size() <= 1)

  // P-15: presenting a token that was already rotated, while it is still
  // stored, is treated as reuse.
  val reuseAfterRotationIsDetected =
    REFRESHES.forall(p =>
      (refreshPc.get(p) == "done" and sawRotated.get(p)) implies refreshOut.get(p) == "reuse")

  // P-15: once reuse is detected, no session is left live. (Nothing but a
  // rotation mints a token here, and a rotation needs an unrevoked token.)
  val reuseRevokesEverySession =
    reuseDetected implies TOKEN_IDS.forall(t => not(live(t)))

  val safety = and {
    atMostOneSuccessorPerToken,
    reuseAfterRotationIsDetected,
    reuseRevokesEverySession,
  }

  // Not part of `safety`: a stronger reading of P-15 that the code does NOT
  // keep, kept to state the bound. Once a rotated token expires and cleanup
  // deletes it, presenting it is refused as unknown ("invalid"), not treated
  // as reuse, so the user's later sessions are not revoked. It can't yield a
  // session either way.
  val everyReplayOfARotatedTokenIsDetected =
    REFRESHES.forall(p =>
      (refreshPc.get(p) == "done" and replayedRotated.get(p)) implies refreshOut.get(p) == "reuse")
}
