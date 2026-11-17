"""Schemathesis hooks for the API fuzz run (OT-011). Loaded by run.sh.

FUZZ_PASS selects the pass (design Decision 5):

- ``visitor``: no credentials, every operation.
- ``signed-in``: a Member on Member routes, a Super Admin on ``/admin/*``.
- ``member-on-admin``: a Member on ``/admin/*``; everything must be refused.

Sessions come from the real sign-up flow (dev mode: the fixed code for
``test-*`` addresses). When a request ends the session (sign-out, password
change, account deletion, refresh-token reuse), the next 401 makes Schemathesis
invalidate the cached credentials and call ``get`` again, which signs up a *new*
Member: the old one may have a changed password or be scheduled for deletion.
"""

from __future__ import annotations

import hashlib
import itertools
import json
import os
import re
import secrets
from dataclasses import dataclass
from typing import Any

import requests
import schemathesis

PASS = os.environ.get("FUZZ_PASS", "visitor")
BASE_URL = os.environ.get("FUZZ_BASE_URL", "http://127.0.0.1:4399").rstrip("/")
API = f"{BASE_URL}/api/v1"
DATABASE_URL = os.environ.get("FUZZ_DATABASE_URL", "")
# Every sign-up costs an Argon2 hash; a run that keeps losing its session is
# broken, not unlucky.
MAX_SIGNUPS = int(os.environ.get("FUZZ_MAX_SIGNUPS", "60"))
RUN_ID = os.environ.get("FUZZ_RUN_ID") or secrets.token_hex(4)

DEV_TEST_CODE = "123456"  # dev mode's fixed code for test-* addresses
PASSWORD = "fuzz-test-password-1"
FINGERPRINT = "apifuzz-device"
ADMIN_PREFIX = "/admin/"

AUTH_COOKIE = "__Host-auth_token"
REFRESH_COOKIE = "__Host-refresh_token"
CSRF_COOKIE = "__Host-csrf_token"
CSRF_HEADER = "X-CSRF-Token"


class FuzzSetupError(RuntimeError):
    """The harness couldn't get a session; the run must fail, not skip."""


# ── Sessions ─────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Session:
    email: str
    cookies: dict[str, str]
    csrf: str


_signups = itertools.count(1)
signup_log: list[str] = []


def _post(http: requests.Session, path: str, body: dict[str, Any], *, fingerprint: bool = False) -> requests.Response:
    headers = {"X-Device-Fingerprint": FINGERPRINT} if fingerprint else {}
    resp = http.post(f"{API}{path}", json=body, headers=headers, timeout=30)
    if resp.status_code != 200:
        raise FuzzSetupError(f"POST {path} returned {resp.status_code}: {resp.text[:300]}")
    return resp


def _session_from(http: requests.Session, resp: requests.Response, email: str) -> Session:
    jar = http.cookies.get_dict()
    missing = [c for c in (AUTH_COOKIE, REFRESH_COOKIE, CSRF_COOKIE) if c not in jar]
    if missing:
        raise FuzzSetupError(f"sign-in for {email} set no {missing} cookie")
    return Session(
        email=email,
        cookies={c: jar[c] for c in (AUTH_COOKIE, REFRESH_COOKIE, CSRF_COOKIE)},
        csrf=resp.json()["csrf_token"],
    )


def sign_up_member() -> Session:
    """Register, verify and complete sign-up for a fresh test Member."""
    n = next(_signups)
    if n > MAX_SIGNUPS:
        raise FuzzSetupError(
            f"more than {MAX_SIGNUPS} sign-ups in one run (FUZZ_MAX_SIGNUPS); sessions are being lost too often"
        )
    email = f"test-fuzz-{RUN_ID}-{n}@example.test"
    http = requests.Session()
    _post(http, "/auth/register", {"email": email})
    verified = _post(http, "/auth/verify-email", {"email": email, "code": DEV_TEST_CODE})
    token = verified.json().get("signup_token")
    if not token:
        raise FuzzSetupError(f"verify-email for {email} returned no signup_token")
    done = _post(
        http,
        "/auth/complete-signup",
        {"signup_token": token, "email": email, "password": PASSWORD},
        fingerprint=True,
    )
    signup_log.append(email)
    print(f"apifuzz: signed up {email}", flush=True)
    if os.environ.get("FUZZ_TRACE"):
        print(f"TRACE session token {http.cookies.get(AUTH_COOKIE, '')[:40]!r}", flush=True)
    return _session_from(http, done, email)


def promote(email: str) -> None:
    """Make the account a Super Admin, as `just grant-admin` does, on the fuzz database."""
    import psycopg

    if not DATABASE_URL:
        raise FuzzSetupError("FUZZ_DATABASE_URL is not set; can't promote the Admin")
    with psycopg.connect(DATABASE_URL, autocommit=True) as conn:
        updated = conn.execute(
            "UPDATE users SET role = 'super_admin' WHERE email = %s", (email,)
        ).rowcount
    if updated != 1:
        raise FuzzSetupError(f"promoting {email} updated {updated} rows")


def sign_up_admin() -> Session:
    member = sign_up_member()
    promote(member.email)
    # Sign in again so the session token carries the new role.
    http = requests.Session()
    resp = _post(http, "/auth/login", {"email": member.email, "password": PASSWORD}, fingerprint=True)
    return _session_from(http, resp, member.email)


def _security(operation: Any) -> list[dict[str, Any]]:
    return operation.definition.raw.get("security") or []


def _mentions(operation: Any, scheme: str) -> bool:
    """Some security requirement of `operation` uses `scheme` (it may be optional)."""
    return any(scheme in r for r in _security(operation))


def _own_method(case: Any) -> bool:
    """The case uses its operation's method. The coverage phase also sends other
    methods (OPTIONS is a CORS preflight; the rest get 405), which the custom
    auth checks don't judge; `error_body` still does."""
    return case.method.upper() == case.operation.method.upper()


def _requires(operation: Any, scheme: str) -> bool:
    """Every security requirement of `operation` uses `scheme`."""
    requirements = _security(operation)
    return bool(requirements) and all(scheme in r for r in requirements)


# ── Auth provider ────────────────────────────────────────────────────────────


def is_admin_operation(path: str) -> bool:
    return path.startswith(ADMIN_PREFIX) or f"/api/v1{ADMIN_PREFIX}" in path


def _identity_for(operation: Any) -> str:
    """Which cached session an operation uses. Public operations get their own
    key: the provider returns no credentials for them, and Schemathesis caches
    that `None` per key, so sharing the Member's key would sign out every
    Member operation after the first public one."""
    if not _mentions(operation, "session_cookie"):
        return "public"
    if PASS == "signed-in" and is_admin_operation(operation.path):
        return "admin"
    return "member"


if PASS in ("signed-in", "member-on-admin"):

    # Cached for the whole run (`None` would mean "never cache"); only a 401
    # invalidates the session, which Schemathesis then replays once.
    @schemathesis.auth(
        refresh_interval=24 * 3600,
        cache_by_key=lambda case, ctx: _identity_for(ctx.operation),
    )
    class FuzzSession:
        """Member credentials (Super Admin on admin routes in the signed-in pass)."""

        def get(self, case: Any, ctx: Any) -> Session | None:
            # Public operations stay credential-free: their own 401s (a wrong
            # password at sign-in) must not look like a lost session.
            identity = _identity_for(ctx.operation)
            if identity == "public":
                return None
            return sign_up_admin() if identity == "admin" else sign_up_member()

        def set(self, case: Any, data: Session, ctx: Any) -> None:
            case.cookies = {**(case.cookies or {}), **data.cookies}
            case.headers = case.headers or {}
            case.headers[CSRF_HEADER] = data.csrf
            case.headers["X-Device-Fingerprint"] = FINGERPRINT


# ── Checks ───────────────────────────────────────────────────────────────────

# Internal text that must never reach a client (design Decision 6). Serde's
# descriptions of the client's own malformed JSON are allowed: they describe
# the request, not the server.
INTERNAL_TEXT = [
    re.compile(p, re.IGNORECASE)
    for p in (
        r"sqlx",
        r"SQLSTATE",
        r"duplicate key",
        r"\bviolates\b",
        r'relation "',
        r"panicked",
        r"src/\S+\.rs:",
        r"RUST_BACKTRACE",
        r"Internal error:",
        r"Database error:",
    )
]


def leaked_internal_text(message: str, request_text: str = "") -> str | None:
    """The first internal-text pattern `message` matches, if any. A match that
    is the client's own input echoed back ("Unknown provider: SQLSTATE" for the
    path `/oauth/SQLSTATE/unlink`) describes the request, not the server."""
    for pattern in INTERNAL_TEXT:
        for match in pattern.finditer(message):
            if match.group(0).lower() not in request_text.lower():
                return pattern.pattern
    return None


def _header(response: Any, name: str) -> str:
    for key, values in response.headers.items():
        if key.lower() == name:
            return values[0] if values else ""
    return ""


def error_body_problem(status: int, content_type: str, body: Any, request_text: str = "") -> str | None:
    """Why an error response breaks the error contract, or None if it doesn't."""
    if status < 400:
        return None
    if status in (415, 422):
        return f"malformed input must be refused with 400, not {status}"
    if not content_type.lower().startswith("application/json"):
        return f"{status} response is {content_type or 'missing a content type'}, not JSON"
    if not isinstance(body, dict):
        return f"{status} body is not a JSON object"
    if isinstance(body.get("code"), str) and isinstance(body.get("message"), str):
        text = body["message"]
    elif isinstance(body.get("error"), str) and isinstance(body.get("error_code"), str):
        text = body["error"]  # the verification error shape
    else:
        return f"{status} body is neither the standard nor the verification error shape: {body!r}"
    leak = leaked_internal_text(text, request_text)
    if leak:
        return f"{status} message contains internal text ({leak!r}): {text!r}"
    return None


def _request_text(response: Any) -> str:
    """The request as the client sent it (decoded URL and body), to tell echoed
    input from internal text."""
    from urllib.parse import unquote

    request = response.request
    body = request.body or b""
    if isinstance(body, bytes):
        body = body.decode("utf-8", "replace")
    return f"{unquote(request.url or '')}\n{body}"


@schemathesis.check
def error_body(ctx: Any, response: Any, case: Any) -> bool | None:
    """Every error is JSON in a documented shape, and never carries internal text."""
    if response.status_code < 400:
        return None
    try:
        body = response.json()
    except ValueError:
        body = None
    problem = error_body_problem(
        response.status_code, _header(response, "content-type"), body, _request_text(response)
    )
    if problem:
        raise AssertionError(problem)
    return None




if PASS == "signed-in":

    @schemathesis.check
    def csrf_enforced(ctx: Any, response: Any, case: Any) -> bool | None:
        """A request that succeeded with the CSRF header is refused with 403 without it."""
        if not (_own_method(case) and 200 <= response.status_code < 300 and _requires(case.operation, "csrf_header")):
            return None
        headers = {k: v for k, v in (case.headers or {}).items() if k.lower() != CSRF_HEADER.lower()}
        probe = case.derive(headers=headers, cookies=dict(case.cookies or {})).call(base_url=BASE_URL)
        if probe.status_code != 403:
            raise AssertionError(
                f"without {CSRF_HEADER}, {case.method} {case.formatted_path} returned "
                f"{probe.status_code}, not 403"
            )
        return None


if PASS == "member-on-admin":

    @schemathesis.check
    def admin_refused(ctx: Any, response: Any, case: Any) -> bool | None:
        """A Member gets 403 on every admin route, whatever the input."""
        # Probes and generated cases without the Member's session are the
        # Visitor pass's business (and get 401).
        if not _own_method(case) or AUTH_COOKIE not in (case.cookies or {}):
            return None
        if response.status_code != 403:
            raise AssertionError(
                f"Member got {response.status_code} on {case.method} {case.formatted_path}, not 403"
            )
        return None


SERVER_ID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}")  # UUIDv7


@schemathesis.check
class request_report:  # noqa: N801 - the check's name in Schemathesis output
    """Print how many requests each operation got, and a digest of the generated
    inputs: the same seed on the same commit must print the same digest.

    Keyed by case ID, so shrinking and replays (which re-run `after_response`)
    don't count twice. Credentials are left out of the digest (they're fresh
    every run)."""

    def __init__(self) -> None:
        self.cases: dict[str, set[str]] = {}
        self.succeeded: dict[str, set[str]] = {}
        self.inputs: set[str] = set()

    def after_response(self, ctx: Any, response: Any, case: Any) -> None:
        if os.environ.get("FUZZ_TRACE") and os.environ["FUZZ_TRACE"] in case.operation.label:
            print(f"TRACE {case.method} {case.formatted_path} -> {response.status_code} "
                  f"auth={str((case.cookies or {}).get(AUTH_COOKIE))[:40]!r} body={response.content[:120]!r}", flush=True)
        self.cases.setdefault(case.operation.label, set()).add(case.id)
        if _own_method(case) and 200 <= response.status_code < 400:
            self.succeeded.setdefault(case.operation.label, set()).add(case.id)
        generated = {
            "method": case.method,
            "path": case.formatted_path,
            "query": case.query,
            # No body is a `NotSet` sentinel, whose repr holds a memory address.
            "body": case.body if isinstance(case.body, (dict, list, str, int, float, bool, type(None))) else None,
            "media_type": case.media_type,
        }
        # IDs learned from earlier responses (a real user's UUID in a role
        # change) are assigned by the server and differ on every fresh database.
        self.inputs.add(SERVER_ID.sub("<id>", json.dumps(generated, sort_keys=True, default=repr)))

    def after_run(self, ctx: Any) -> None:
        width = max((len(label) for label in self.cases), default=0)
        print(f"\napifuzz: requests per operation (pass {PASS}; requests, of which 2xx/3xx):", flush=True)
        for label in sorted(self.cases):
            ok = len(self.succeeded.get(label, ()))
            print(f"  {label:<{width}}  {len(self.cases[label]):>5}  {ok:>5}", flush=True)
        lines = "\n".join(sorted(self.inputs))
        if dump := os.environ.get("FUZZ_DUMP_INPUTS"):
            with open(f"{dump}.{PASS}", "w") as f:
                f.write(lines)
        digest = hashlib.sha256(lines.encode()).hexdigest()[:16]
        print(f"apifuzz: {sum(map(len, self.cases.values()))} requests, input digest {digest}", flush=True)
