# Design

## Context

See proposal.md for why. The current state that shapes the approach:

- `DashboardLayout.astro` renders one island, `DashboardApp` (`client:load`):
  `AuthGuard` → `DashboardShell` (`AuthProvider` + header + `DashboardNav`) →
  `<slot />`, where the slot is the page's panel island (`HomePanel`,
  `SettingsPanel`, `AdminPanel`). `AuthGuard` renders a spinner until `onMount`
  has the user, then renders the slot.
- `Base.astro` already uses Astro's `ClientRouter`, so dashboard links are
  client-side navigations. The JavaScript realm survives them and module state
  persists. Only the DOM is swapped, so `DashboardApp` remounts on every move.
- The user lives in three places: `lib/auth/session.ts` (sessionStorage cache,
  per tab), `lib/auth/current-user.ts` (non-reactive module variable) and
  `AuthProvider`'s Svelte context (read only by `DashboardNav`).
- The session hint already exists. `lib/auth/token.ts` keeps the JWT expiry in
  localStorage (`jwt_expiry`), set on login and refresh and cleared on logout or
  failed refresh. The JWT and refresh token are HttpOnly cookies.
- `HttpClient` already handles the failure cases for any authenticated call. A
  401 refreshes, then redirects to `/login?session=expired` if that fails. A 403
  `email_not_verified` redirects to `/verify-email`.
- `DashboardNav` already tracks the path via `astro:page-load`.
- Inline scripts are CSP-pinned by sha256 (`apps/web/Caddyfile`). External
  same-origin scripts are allowed (`public/scripts/theme-init.js` is the
  pattern for "must run before paint").

## Goals / Non-Goals

**Goals:**
- No full-screen loading state on any dashboard page for a browser with a
  session hint. The account check runs once per document, in the background.
- Each wrong-guess case ends in the same place as today (sign-in, verify email,
  Home), and no server data is shown to the wrong account.

**Non-Goals:**
- Optimistic mutations (a separate change, see proposal).
- Changing `HttpClient`'s refresh or CSRF logic. Only the shape of the errors
  it throws changes (D8).

## Hypothesis

We believe the static dashboard HTML exposes nothing (all data comes from
authorised API calls). If so, showing it optimistically is safe, and the only
visible cost of a wrong guess is a brief flash of chrome before a redirect. The
first tasks test this with Playwright against the mocked API: the
happy-path test (no spinner, header node survives moves, one `/auth/me`) plus
one test per wrong guess (no hint, 401, unverified, non-admin on Admin, 500).
We'll know it worked when the wrong-guess tests show no server data and no Admin
entry for a Member at any point, and the happy path never renders "Loading...".
Being wrong is cheap (frontend-only, revert by digest), so CI is the right place
to test.

**Result (2026-11-12):** it holds. Every wrong-guess test and the happy path
pass in `just ci` (`dashboard.spec.ts`, `dashboard-optimistic.spec.ts`), and a
manual check in `just dev` on desktop and phone width, including Slow 3G,
showed no spinner, no header flicker, correct placeholders and a correctly
late Admin entry. No decisions changed. There is no production deployment yet,
so the planned day-in-production check didn't happen. Repeat the 1.13 on-site
checks after the first deploy.

## Decisions

### D1. Flat layout: a persisted header island, panels directly in `<main>`
```astro
<Base {title}>
  <script is:inline slot="head" src="/scripts/dashboard-session-hint.js"></script>
  <DashboardHeader client:load transition:persist="dashboard-header" />
  <main class="mx-auto w-full max-w-6xl px-4 py-6"><slot /></main>
</Base>
```
There is no gate around the content. `transition:persist` keeps an island's DOM
and state, including its slot, so the persisted island must not contain the
page content. Panels are ordinary per-page islands and are rebuilt on each move.

*Alternatives:* a non-persisted `SessionGate` island wrapping the slot (the
first draft). It is pessimistic by construction and nests islands, so it was
dropped. Persisting today's `DashboardApp` would keep the old panel.

### D2. Session hint check before paint, in an external head script
`public/scripts/dashboard-session-hint.js` runs render-blocking in `<head>` on
dashboard pages. If `localStorage.jwt_expiry` is absent, it calls
`location.replace('/login')`. This is a local read with no network call, so
Visitors never see the dashboard. An expired-but-present hint still counts: the
refresh token may still work, and the background check settles it.

The script is external (`'self'`), so no new CSP hash is needed. `Base.astro`
gains a named `head` slot. The `jwt_expiry` key name becomes a shared constant
documented in both files, since the script cannot import TS. On client-side
navigation into the dashboard, Astro does not re-run head scripts, so the
session store (D3) repeats the same check on init as a backstop.

*Alternative:* do the check in the header island. It runs after hydration, so
Visitors would see the chrome flash on every visit.

### D3. Reactive session store, `lib/auth/session.svelte.ts`
Module-level runes state, shared by every island on the page (one module
instance per document; the one-`/auth/me` test confirms this):

- `user: User | null`: initialised synchronously from the sessionStorage cache
  (stale is fine), else `null`.
- `status: 'unconfirmed' | 'confirmed' | 'unreachable'`.
- `revalidate()`: single-flight, at most once per document unless retried. It
  calls `api.auth.me()`. On success it sets `user` (write-through to
  sessionStorage) and `status = 'confirmed'`, and redirects to
  `/verify-email?email=…` if unverified. On a network or 5xx error it sets
  `status = 'unreachable'` and stays on the page. A 401 or 403 never returns:
  `HttpClient` has already redirected.
- `retry()`: resets and calls `revalidate()` again (used by the notice).
- `patch(partial)` / `set(user)`: update and write through. Panels call these
  after successful actions.
- `signOut()`: see D6.

The header calls `revalidate()` on mount. Panels never trigger it themselves;
they just read `user`.

Replaces `current-user.ts`, `AuthGuard.svelte`, `AuthProvider.svelte`,
`DashboardApp.svelte`, `DashboardShell.svelte`, and the `AuthContext` /
`AUTH_CONTEXT_KEY` exports. `session.ts` stays as the sessionStorage layer under
the store. `GuestGuard` keeps reading it, and `HttpClient` keeps calling
`clearSessionUser` before its full-page redirects (which reset the store).

*Alternatives:* Svelte context (cannot cross islands); a `writable` store
(runes are the project standard); keep the `/auth/me` result off the critical
path entirely and rely on panels' own calls (the header and Settings need
`role`, `has_password` and `oauth_providers`, which only `/auth/me` gives).

### D4. Placeholders, not spinners, for unknown account details
Where a panel or the header needs `user` and it is `null`, it renders a sized
skeleton (email line, Settings sections) in place of the content. Settings
renders its forms only once `user` is known, because which form to show depends
on `has_password`. The "unreachable" notice is a non-blocking banner under the
header with a Retry button, rendered by the header island.

### D5. Admin: never guess "admin"
- The nav shows the Admin entry only when `user?.role` is admin or super_admin.
  An unknown role means no entry, so a Member never sees it flash. An Admin with
  no cached user gets the entry once the check answers.
- `AdminPanel` calls `api.admin.listUsers()` immediately (optimistic: the server
  decides) and shows a skeleton table until it answers. On 403 it calls
  `navigateReplace('/dashboard')`. It also redirects if the store confirms a
  non-admin role first. Its current `onMount` role check is removed.

### D6. Sign-out: clear locally first, bounded wait for the server
`signOut()` clears `user`, sessionStorage and `jwt_expiry` first, so D2 sends
any later dashboard visit to `/login`. It then awaits `api.auth.logout()` raced
against a 1-second timeout and calls `navigateReplace('/login')`. The request
uses `keepalive: true`, so it can still complete if the timeout wins.

This is deliberately less optimistic than the rest. If the logout request
never reaches the server, the refresh cookie stays valid until it expires, so
we give it a short, bounded chance to land. With the local hint cleared, the
dashboard won't use that cookie anyway.

*Found while implementing:* a logout POST with no CSRF cookie first runs a
token refresh, which writes `jwt_expiry` back. If logout then hangs, the hint
survives sign-out and the dashboard opens optimistically again. `signOut()`
therefore clears the cache and hint a second time, after the race and right
before navigating. A refresh that resolves in the brief gap between that clear
and the page unloading can still restore the hint. In that case the server
session really was refreshed and not yet ended, so the next dashboard visit
behaves as signed in until the `keepalive` logout lands.

*Alternatives:* navigate immediately and fire-and-forget (fully optimistic;
risks a live refresh cookie on a slow network); await without a bound (today's
behaviour; a hung server hangs sign-out).

### D7. Hydration directive rule (documentation only)
Primary page content stays `client:load`. Content that starts below the fold
uses `client:visible`. No current page has any. Recorded in the Frontend rules
in `CLAUDE.md`, including that the first `client:visible` adds an inline
hydration script, which needs its sha256 added to `apps/web/Caddyfile`.

### D8. Errors thrown by `HttpClient` carry the HTTP status
Today a non-OK response whose body has no `error_code` becomes a plain `Error`
with only a message. The backend's canonical body is `{ code, message }`, so
this covers 403 `forbidden` and every 5xx. D3 (5xx → "unreachable") and D5
(403 → Home) need the status. `HttpClient` throws `ApiErrorException` for
every non-OK response, with a `status` field and the parsed body when there is
one. The message still includes the trace ID, as the plain `Error` does today.
A fetch that never gets a response keeps throwing the browser's `TypeError`,
which D3 treats as unreachable. Add an `isApiStatus(err, status)` helper in
`errors.ts`. Callers that read `.message` or check `'error_code' in err` keep
working, because the class still extends `Error` and copies body fields onto
itself.

*Alternatives:* match on message text (brittle); a separate `HttpError` class
(two error types for callers to narrow instead of one).

## Risks / Trade-offs

- [A stale cache shows outdated details (old email, removed Google link) for a
  moment] → The background check corrects them within one round trip. The
  server authorises every action, so acting on stale details fails safely.
- [Expired session: chrome and cached details flash before the redirect] →
  Accepted trade-off of the optimistic model. The cache is per tab
  (sessionStorage) and only ever holds this tab's own account. No server data
  is fetched successfully.
- [Two copies of the store module in different chunks] → The one-`/auth/me`
  test fails if so. If it does, route all imports through one shared entry.
- [Head script not re-run on client-side navigation into the dashboard from a
  public page] → The store's init repeats the hint check (D2).
- [Client-side navigation from a public page into the dashboard with no hint
  briefly shows the dashboard chrome before redirecting] → Seen in testing:
  Astro does re-run the new head script on the swap, but only after the new
  body is in place. No account data is shown and no `/auth/me` is made. Public
  pages don't link into the dashboard for Visitors, so it is only reachable by
  hand. Accepted.
- [`keepalive` logout dropped by the browser] → Bounded 1 s await first. The
  session also ends when the refresh token expires, and the local hint is
  already gone.
- [Overlap with OT-002, which rewrites `ApiError` in `errors.ts` as a
  generated type] → D8 only adds `status` to the exception class and one
  helper. Whichever change lands second rebases onto the other; the conflict
  is small.
- [Tests built on `AuthProvider`/`AuthGuard` wrappers] → Replaced in the same
  slice.

## Migration Plan

Frontend-only. It ships as one image; rollback is reverting the image digest in
the deploy config. No data or API changes, and no flag: every wrong-guess path
has a mocked-API Playwright test, and reverting is cheap.
