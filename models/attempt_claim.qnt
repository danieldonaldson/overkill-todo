// -*- mode: Bluespec; -*-
//
// The code-entry protocol under concurrency (OT-010 design D8; threat model
// P-11, P-12). One user's one-time code row, raced by guesses, code-issuing
// requests and the expiry cleanup job.
//
// Each request is the sequence of atomic database steps it really runs. One
// model step is one SQL statement, so any interleaving of the requests' steps
// is a schedule Postgres can produce. Work between statements (hashing,
// comparing, the reissue decision) belongs to the step before it, because it
// touches no shared state.
//
//   guess  (verify-email / verify-reset-code):
//     read     find_by_user; an absent or expired code gets "nocode"
//     claim    register_attempt: UPDATE … SET attempts = attempts + 1
//              WHERE … AND attempts < 5, reporting Claimed / Locked / NotFound
//     compare  the Argon2 comparison against the code read (no database)
//     release  release_attempt, after a correct guess
//     consume  delete_for_user (email), or consume(user, code id) (reset)
//   issue  (resend-verification / register / forgot-password):
//     decide   find_by_user, then the domain's reissue(now)
//     write    the issuing upsert (save). The new code is stamped with the
//              time of the write (it is built just before it, after hashing),
//              and the upsert re-checks the reissue rule at that time: no
//              replacing a code locked inside its window (task 4.1) or still
//              inside the resend cooldown
//     deliver  the email; if it fails (registration only), set_last_sent_at
//              moves the issued code's send time back past the cooldown, so
//              the user can ask again at once. Only that code, below the cap.
//   cleanup  delete_expired (one statement)
//   tick     time passes
//
// Time is whole minutes. The database clock is a hair later than the model's
// `now` (the test that replays these traces takes seconds), so a comparison
// with NOW() that is `x < NOW()` in SQL is `x <= now` here.
//
// Traces of this model are replayed against the real repositories by
// backend/crates/server/tests/model_conformance.rs (`just models`).
module attempt_claim {
  // ── Variants ────────────────────────────────────────────────────────────

  // The issuing upsert refuses to replace a code that is at the cap and
  // inside its lock window (task 4.1). `false` is the old unguarded upsert.
  const GUARDED_UPSERT: bool
  // The issuing upsert also refuses inside the resend cooldown. `false` is
  // the first version of the task 4.1 guard, which checked only the lock.
  const GUARD_COOLDOWN: bool
  // A failed delivery releases the cooldown (registration's verification
  // email; password reset has no such path).
  const RELEASE_ON_FAILED_DELIVERY: bool
  // The attempt claim and release name the code id the guess read, so a
  // guess whose code was replaced claims nothing. `false` is the old claim by
  // user only.
  const CLAIM_BY_CODE_ID: bool
  // A correct guess removes the code by its id (password reset: consume)
  // rather than by user (email verification: delete_for_user).
  const CONSUME_BY_CODE_ID: bool
  // Minutes a code is live: 1440 for email verification, 15 for reset.
  const EXPIRY: int

  // ── Policy (domain/entities/attempt_lockout.rs) ─────────────────────────

  pure val MAX_ATTEMPTS = 5
  pure val COOLDOWN = 5
  // LOCKOUT_MEMORY_DAYS (30) in minutes.
  pure val MEMORY = 43200

  // LOCK_WINDOWS_MINUTES: 15 doubled per earlier lockout, at most 24 hours.
  pure def window(lockouts: int): int =
    if (lockouts <= 0) 15
    else if (lockouts == 1) 30
    else if (lockouts == 2) 60
    else if (lockouts == 3) 120
    else if (lockouts == 4) 240
    else if (lockouts == 5) 480
    else if (lockouts == 6) 960
    else 1440

  // ── State ───────────────────────────────────────────────────────────────

  type Row = {
    present: bool,
    id: int,
    attempts: int,
    lockouts: int,
    sent: int,
    expires: int,
  }

  // `flag`: the guess was correct (guessCompare), or the email failed
  // (issueDeliver).
  type Last = { act: str, who: int, flag: bool, delta: int }

  pure val GUESSES = Set(1, 2, 3)
  pure val ISSUES = Set(1, 2)
  // Code ids: 0 is the initial code; each issue mints at most one more.
  pure val CODE_IDS = 0.to(ISSUES.size())

  var row: Row
  var now: int
  var nextId: int

  var guessPc: int -> str
  var guessCode: int -> int
  var guessOut: int -> str
  var issuePc: int -> str
  var issueCode: int -> int
  var issueOut: int -> str

  // Ghosts: what the invariants watch. The database can't see them.
  // Wrong guesses compared against each code.
  var wrong: int -> int
  // A code locked inside its window was replaced.
  var lockLifted: bool
  // A guess whose code was already gone was told "locked".
  var misleadingLock: bool
  // A code was replaced inside the resend cooldown.
  var cooldownSkipped: bool

  // The step just taken, for the conformance replay.
  var last: Last

  // ── Steps ───────────────────────────────────────────────────────────────

  pure def did(act: str, who: int): Last =
    { act: act, who: who, flag: false, delta: 0 }

  // The domain's reissue decision (`reissue_at`) on a stored row.
  def reissueAllowed(r: Row, t: int): bool = and {
    not(r.attempts >= MAX_ATTEMPTS and t < r.sent + window(r.lockouts)),
    t >= r.sent + COOLDOWN,
  }

  // The stored row is the one guess `g` read, or (claim by user) any row.
  def claimTarget(g: int): bool =
    row.present and (not(CLAIM_BY_CODE_ID) or row.id == guessCode.get(g))

  action guessRead(g: int): bool = all {
    guessPc.get(g) == "read",
    if (row.present and now < row.expires) all {
      guessPc' = guessPc.set(g, "claim"),
      guessCode' = guessCode.set(g, row.id),
      guessOut' = guessOut,
    } else all {
      guessPc' = guessPc.set(g, "done"),
      guessCode' = guessCode,
      guessOut' = guessOut.set(g, "nocode"),
    },
    row' = row, now' = now, nextId' = nextId,
    issuePc' = issuePc, issueCode' = issueCode, issueOut' = issueOut,
    wrong' = wrong, lockLifted' = lockLifted, misleadingLock' = misleadingLock,
    cooldownSkipped' = cooldownSkipped,
    last' = did("guessRead", g),
  }

  action guessClaim(g: int): bool = {
    val target = claimTarget(g)
    val claimed = target and row.attempts < MAX_ATTEMPTS
    // register_attempt's answer: Claimed, else Locked if its row exists, else
    // NotFound (the no-code answer).
    val outcome = if (claimed) "claimed" else if (target) "locked" else "nocode"
    all {
      guessPc.get(g) == "claim",
      row' = if (claimed) row.with("attempts", row.attempts + 1) else row,
      guessPc' = guessPc.set(g, if (claimed) "compare" else "done"),
      guessOut' = if (claimed) guessOut else guessOut.set(g, outcome),
      misleadingLock' = (misleadingLock or (outcome == "locked"
        and not(row.present and row.id == guessCode.get(g)))),
      now' = now, nextId' = nextId, guessCode' = guessCode,
      issuePc' = issuePc, issueCode' = issueCode, issueOut' = issueOut,
      wrong' = wrong, lockLifted' = lockLifted, cooldownSkipped' = cooldownSkipped,
      last' = did("guessClaim", g),
    }
  }

  action guessCompare(g: int, correct: bool): bool = all {
    guessPc.get(g) == "compare",
    if (correct) all {
      guessPc' = guessPc.set(g, "release"),
      guessOut' = guessOut,
      wrong' = wrong,
    } else all {
      guessPc' = guessPc.set(g, "done"),
      guessOut' = guessOut.set(g, "invalid"),
      wrong' = wrong.setBy(guessCode.get(g), w => w + 1),
    },
    row' = row, now' = now, nextId' = nextId, guessCode' = guessCode,
    issuePc' = issuePc, issueCode' = issueCode, issueOut' = issueOut,
    lockLifted' = lockLifted, misleadingLock' = misleadingLock,
    cooldownSkipped' = cooldownSkipped,
    last' = { act: "guessCompare", who: g, flag: correct, delta: 0 },
  }

  action guessRelease(g: int): bool = all {
    guessPc.get(g) == "release",
    row' = if (claimTarget(g) and row.attempts > 0)
        row.with("attempts", row.attempts - 1)
      else row,
    guessPc' = guessPc.set(g, "consume"),
    now' = now, nextId' = nextId, guessCode' = guessCode, guessOut' = guessOut,
    issuePc' = issuePc, issueCode' = issueCode, issueOut' = issueOut,
    wrong' = wrong, lockLifted' = lockLifted, misleadingLock' = misleadingLock,
    cooldownSkipped' = cooldownSkipped,
    last' = did("guessRelease", g),
  }

  action guessConsume(g: int): bool = all {
    guessPc.get(g) == "consume",
    row' = if (row.present and (not(CONSUME_BY_CODE_ID) or row.id == guessCode.get(g)))
        row.with("present", false)
      else row,
    guessPc' = guessPc.set(g, "done"),
    guessOut' = guessOut.set(g, "verified"),
    now' = now, nextId' = nextId, guessCode' = guessCode,
    issuePc' = issuePc, issueCode' = issueCode, issueOut' = issueOut,
    wrong' = wrong, lockLifted' = lockLifted, misleadingLock' = misleadingLock,
    cooldownSkipped' = cooldownSkipped,
    last' = did("guessConsume", g),
  }

  action issueDecide(i: int): bool = all {
    issuePc.get(i) == "decide",
    issuePc' = issuePc.set(i,
      if (not(row.present) or reissueAllowed(row, now)) "write" else "done"),
    issueOut' = issueOut.set(i,
      if (not(row.present) or reissueAllowed(row, now)) "" else "withheld"),
    issueCode' = issueCode,
    row' = row, now' = now, nextId' = nextId,
    guessPc' = guessPc, guessCode' = guessCode, guessOut' = guessOut,
    wrong' = wrong, lockLifted' = lockLifted, misleadingLock' = misleadingLock,
    cooldownSkipped' = cooldownSkipped,
    last' = did("issueDecide", i),
  }

  action issueWrite(i: int): bool = {
    val atCap = row.present and row.attempts >= MAX_ATTEMPTS
    val inLock = atCap and now < row.sent + window(row.lockouts)
    val inCooldown = row.present and now < row.sent + COOLDOWN
    // The SQL guard, at the new code's send time (now).
    val refused = (GUARDED_UPSERT and inLock) or (GUARD_COOLDOWN and inCooldown)
    all {
      issuePc.get(i) == "write",
      if (refused) all {
        row' = row,
        nextId' = nextId,
        issuePc' = issuePc.set(i, "done"),
        issueCode' = issueCode,
        issueOut' = issueOut.set(i, "refused"),
        lockLifted' = lockLifted,
        cooldownSkipped' = cooldownSkipped,
      } else all {
        row' = {
          present: true,
          id: nextId,
          attempts: 0,
          lockouts: if (row.present) row.lockouts + (if (atCap) 1 else 0) else 0,
          sent: now,
          expires: now + EXPIRY,
        },
        nextId' = nextId + 1,
        issuePc' = issuePc.set(i, "deliver"),
        issueCode' = issueCode.set(i, nextId),
        issueOut' = issueOut.set(i, "issued"),
        lockLifted' = (lockLifted or inLock),
        cooldownSkipped' = (cooldownSkipped or inCooldown),
      },
      now' = now,
      guessPc' = guessPc, guessCode' = guessCode, guessOut' = guessOut,
      wrong' = wrong, misleadingLock' = misleadingLock,
      last' = did("issueWrite", i),
    }
  }

  action issueDeliver(i: int, failed: bool): bool = {
    // set_last_sent_at: WHERE user AND id = the issued code AND attempts < 5.
    val releases = RELEASE_ON_FAILED_DELIVERY and failed and row.present
      and row.id == issueCode.get(i) and row.attempts < MAX_ATTEMPTS
    all {
      issuePc.get(i) == "deliver",
      issuePc' = issuePc.set(i, "done"),
      row' = if (releases) row.with("sent", now - COOLDOWN) else row,
      // Moving a locked code's send time back would shorten its lock.
      lockLifted' = (lockLifted or (releases and row.attempts >= MAX_ATTEMPTS
        and now < row.sent + window(row.lockouts))),
      now' = now, nextId' = nextId, issueCode' = issueCode, issueOut' = issueOut,
      guessPc' = guessPc, guessCode' = guessCode, guessOut' = guessOut,
      wrong' = wrong, misleadingLock' = misleadingLock, cooldownSkipped' = cooldownSkipped,
      last' = { act: "issueDeliver", who: i, flag: failed, delta: 0 },
    }
  }

  action cleanup: bool = all {
    row' = if (row.present and row.expires <= now
              and (row.lockouts == 0 or row.sent + MEMORY <= now))
        row.with("present", false)
      else row,
    now' = now, nextId' = nextId,
    guessPc' = guessPc, guessCode' = guessCode, guessOut' = guessOut,
    issuePc' = issuePc, issueCode' = issueCode, issueOut' = issueOut,
    wrong' = wrong, lockLifted' = lockLifted, misleadingLock' = misleadingLock,
    cooldownSkipped' = cooldownSkipped,
    last' = did("cleanup", 0),
  }

  action tick(delta: int): bool = all {
    now' = now + delta,
    row' = row, nextId' = nextId,
    guessPc' = guessPc, guessCode' = guessCode, guessOut' = guessOut,
    issuePc' = issuePc, issueCode' = issueCode, issueOut' = issueOut,
    wrong' = wrong, lockLifted' = lockLifted, misleadingLock' = misleadingLock,
    cooldownSkipped' = cooldownSkipped,
    last' = { act: "tick", who: 0, flag: false, delta: delta },
  }

  // ── Initial state: any one code, or none ────────────────────────────────

  action init = {
    nondet present = Bool.oneOf()
    nondet attempts = 0.to(MAX_ATTEMPTS).oneOf()
    nondet lockouts = 0.to(2).oneOf()
    // Minutes since the code was sent.
    nondet age = Set(0, 3, 6, 14, 20, 40).oneOf()
    all {
      row' = {
        present: present,
        id: 0,
        attempts: attempts,
        lockouts: lockouts,
        sent: 0,
        expires: EXPIRY,
      },
      now' = age,
      nextId' = 1,
      guessPc' = GUESSES.mapBy(_ => "read"),
      guessCode' = GUESSES.mapBy(_ => -1),
      guessOut' = GUESSES.mapBy(_ => ""),
      issuePc' = ISSUES.mapBy(_ => "decide"),
      issueCode' = ISSUES.mapBy(_ => -1),
      issueOut' = ISSUES.mapBy(_ => ""),
      // The initial code's attempts so far were wrong guesses.
      wrong' = CODE_IDS.mapBy(c => if (c == 0 and present) attempts else 0),
      lockLifted' = false,
      misleadingLock' = false,
      cooldownSkipped' = false,
      last' = did("init", 0),
    }
  }

  action step = any {
    nondet g = GUESSES.oneOf()
    nondet correct = Bool.oneOf()
    any {
      guessRead(g),
      guessClaim(g),
      guessCompare(g, correct),
      guessRelease(g),
      guessConsume(g),
    },
    nondet i = ISSUES.oneOf()
    nondet failed = Bool.oneOf()
    any { issueDecide(i), issueWrite(i), issueDeliver(i, failed) },
    cleanup,
    nondet delta = Set(1, 5, 10, 15, 60, 1440, 43200).oneOf()
    tick(delta),
  }

  // ── Invariants ──────────────────────────────────────────────────────────

  // P-12: at most 5 wrong guesses are compared against any one code. (A
  // correct guess releases its slot, so it isn't counted.)
  val atMostFiveWrongGuessesPerCode =
    CODE_IDS.forall(c => wrong.get(c) <= MAX_ATTEMPTS)

  // P-12: a guess whose code was replaced, verified or cleaned up gets the
  // no-code answer, never a misleading "too many attempts".
  val goneCodeIsNeverReportedLocked = not(misleadingLock)

  // P-11: a code locked by a guess is never replaced, nor its lock
  // shortened, inside its lock window.
  val lockIsNeverLiftedInsideItsWindow = not(lockLifted)

  // P-11: no code is replaced inside the resend cooldown, however many issuing
  // requests race (each replacement would bring 5 fresh guesses).
  val noReissueInsideTheCooldown = not(cooldownSkipped)

  // A witness, not a property: `just models` runs it as an invariant to get
  // a trace in which the issuing upsert refuses (the race task 4.1 fixed), and
  // replays that trace too, since random traces reach it only rarely.
  val noIssueIsRefused = ISSUES.forall(i => issueOut.get(i) != "refused")

  val safety = and {
    atMostFiveWrongGuessesPerCode,
    goneCodeIsNeverReportedLocked,
    lockIsNeverLiftedInsideItsWindow,
    noReissueInsideTheCooldown,
  }
}

// The code as it stands: email verification codes.
module email_codes {
  import attempt_claim(
    GUARDED_UPSERT = true,
    GUARD_COOLDOWN = true,
    RELEASE_ON_FAILED_DELIVERY = true,
    CLAIM_BY_CODE_ID = true,
    CONSUME_BY_CODE_ID = false,
    EXPIRY = 1440
  ).*
}

// The code as it stands: password reset codes.
module reset_codes {
  import attempt_claim(
    GUARDED_UPSERT = true,
    GUARD_COOLDOWN = true,
    RELEASE_ON_FAILED_DELIVERY = false,
    CLAIM_BY_CODE_ID = true,
    CONSUME_BY_CODE_ID = true,
    EXPIRY = 15
  ).*
}

// Before task 4.1: the issuing upsert replaced a locked code unguarded.
// `lockIsNeverLiftedInsideItsWindow` must fail here (the P-11 race).
module unguarded_upsert {
  import attempt_claim(
    GUARDED_UPSERT = false,
    GUARD_COOLDOWN = false,
    RELEASE_ON_FAILED_DELIVERY = true,
    CLAIM_BY_CODE_ID = true,
    CONSUME_BY_CODE_ID = false,
    EXPIRY = 1440
  ).*
}

// Before slice 4: the attempt claim named only the user. A guess that read a
// code since replaced claims a slot on the new one but compares against the
// old, so `atMostFiveWrongGuessesPerCode` and
// `goneCodeIsNeverReportedLocked` must fail here.
module claim_by_user {
  import attempt_claim(
    GUARDED_UPSERT = true,
    GUARD_COOLDOWN = true,
    RELEASE_ON_FAILED_DELIVERY = true,
    CLAIM_BY_CODE_ID = false,
    CONSUME_BY_CODE_ID = false,
    EXPIRY = 1440
  ).*
}

// The first task 4.1 guard re-checked only the lock, so issuing requests that
// all decided after the cooldown could each replace the code (code review).
// `noReissueInsideTheCooldown` must fail here.
module lock_only_guard {
  import attempt_claim(
    GUARDED_UPSERT = true,
    GUARD_COOLDOWN = false,
    RELEASE_ON_FAILED_DELIVERY = true,
    CLAIM_BY_CODE_ID = true,
    CONSUME_BY_CODE_ID = false,
    EXPIRY = 1440
  ).*
}
