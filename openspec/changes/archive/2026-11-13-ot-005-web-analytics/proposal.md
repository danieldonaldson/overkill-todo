# OT-005: Web analytics

## Why

As the operator, I want to see how Visitors and Members move through the site
(where they arrive from, which pages they use, and where they drop out of
sign-up), so that I can tell which parts of the product work without tracking
anyone personally.

Today the only browser-side measurement is Cloudflare Web Analytics, injected
at Cloudflare's edge. It counts page views and referrers but has no custom
events, so it can't show the sign-up funnel for people who haven't made an
account yet ("opened Register → submitted → verified"). Backend metrics
(OT-001) see requests, not people choosing to leave. This change adds
self-hosted, cookie-free web analytics with events, run by us, so no visitor
data goes to a third party and no consent banner is needed.

## What Changes

- The site records anonymous page views and a small, fixed set of named events
  (for example "sign-up submitted", "email verified", "sign-in started with
  Google") in a self-hosted analytics service.
- Nothing is stored in the browser (no cookies, no local storage), nothing
  identifies a Visitor or Member (no user ID, email or free text in any
  event), and query strings and URL fragments are never recorded, since some
  carry codes and tokens.
- A browser that asks not to be tracked (Do Not Track or Global Privacy
  Control) records nothing.
- All analytics traffic goes through the site's own address, so the site's
  script policy stays as strict as it is now.
- Analytics is off unless configured for an environment. When it is off, or the
  analytics service is down, the site works exactly as before and makes no
  analytics requests or visible errors.
- Only the operator can see the analytics reports. They are not reachable from
  the public site.
- Analytics data older than a fixed period (90 days) is deleted automatically.

## Capabilities

### New Capabilities
- `analytics`: anonymous, privacy-preserving measurement of how Visitors and
  Members use the web app: what is recorded, what never is, who can see it,
  how long it is kept, and how the site behaves when analytics is off or
  unavailable.

### Modified Capabilities
None.

## Appetite

One week.

## Out of Scope

- Business counters in the backend (sign-ups, sign-in outcomes, OAuth
  failures) and their alerts. A separate change.
- Per-Member analytics: DAU/MAU, retention cohorts and feature usage per
  account, which will come from our own database in a later change.
- Session replay, heatmaps, A/B tests, feature flags.
- Removing Cloudflare Web Analytics. It stays until this has run long enough to
  compare.
- A cookie or consent banner. None is needed for this design. If a later
  change wants identified tracking, it must add one.

## Dependencies

- A new self-hosted service (the analytics server and its own database) in the
  local and production deploy config. No change to the product database.
- No production deploy exists yet. Deploy steps are recorded for the first
  deploy.

## Ready

- **Riskiest unknown:** whether analytics served through our own address
  behind a CDN and the edge proxy (Caddy now, Traefik in production) still
  tells Visitors apart. The real client address must reach the analytics
  service, or Visitors with the same browser merge and funnels become
  meaningless. And this must work without the site's script policy or
  existing `/api` routes breaking.
- **First slice:** page views end to end through our own address in the full
  local stack: on when configured and off when not, query strings stripped,
  reports not publicly reachable, with a browser test under the real script
  policy and a check that two different client addresses count as two
  Visitors.

## Impact

`apps/web` (runtime config, one tracking loader, event calls in the auth
forms, CSP test), the edge proxy config (the current Caddy edge now; the
production Traefik edge must meet the same documented contract), `compose.yaml` and `deploy/compose/compose.prod.yaml`
(analytics service, its database and a retention job), the secrets for its
database and app secret, and the README.
