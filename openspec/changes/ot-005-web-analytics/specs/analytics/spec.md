# Spec Delta

## Purpose

Lets the operator measure, anonymously, how Visitors and Members use the web
app (page views, where they arrive from, and how far they get through sign-up
and sign-in) without storing anything in their browsers, identifying anyone,
or sending their data to a third party.

## ADDED Requirements

### Requirement: Page views are recorded anonymously
When analytics is on, the site SHALL record a page view for every page a
Visitor or Member opens, including moves between dashboard pages that don't
reload the page. A page view SHALL record only the page's path, the referring
site, and coarse context derived by the analytics service (approximate location down to
city, browser, operating system, device type, screen size, language). It SHALL NOT record the
query string or the URL fragment.

#### Scenario: Visitor opens a public page
- **WHEN** a Visitor opens the Register page
- **THEN** within 1 minute the operator's report shows one page view of `/register`

#### Scenario: Moving between dashboard pages counts each page
- **WHEN** a Member opens Home and then moves to Settings using the dashboard navigation
- **THEN** the report shows one page view of `/dashboard` and one of `/dashboard/settings`

#### Scenario: Codes and tokens in the address are never recorded
- **WHEN** a Visitor opens a magic sign-in link or password reset link whose address carries a token or code in the query string or fragment
- **THEN** the recorded page view has the path only
- **AND** a search of all stored analytics data finds neither the token nor the code

#### Scenario: Arrival from another site
- **WHEN** a Visitor follows a link to the site from another website
- **THEN** the page view records that website as the referrer

### Requirement: Distinct Visitors are counted without identifying them
The analytics service SHALL count distinct Visitors without cookies, browser
storage, user IDs or stored IP addresses. Two people on different networks
SHALL count as two Visitors, also when the site is behind a CDN or proxy. The
same browser SHALL be recognised as the same Visitor for at most one calendar
month.

#### Scenario: Two Visitors from different networks
- **WHEN** two Visitors on different networks each open the Login page
- **THEN** the report shows 2 Visitors, not 1

#### Scenario: One Visitor, several pages
- **WHEN** one Visitor opens Login, Register and Login again within a few minutes
- **THEN** the report shows 1 Visitor and 3 page views

#### Scenario: Nothing is stored in the browser
- **WHEN** a Visitor opens any page with analytics on
- **THEN** the site sets no cookie and writes no browser storage for analytics

#### Scenario: No IP address is stored
- **WHEN** the operator inspects the stored analytics data
- **THEN** it contains no client IP address

### Requirement: Sign-up and sign-in steps are recorded as named events
When analytics is on, the site SHALL record a named event when a Visitor or
Member completes one of the following steps, so that the operator can build a
funnel from them. The names and property values SHALL come only from this
fixed list; events SHALL NOT carry user IDs, email addresses, names, form
content, error messages or any other free text.

| Event | When | Property |
|---|---|---|
| `signup-submitted` | the server accepts a Register form | none |
| `signup-rejected` | the server rejects a Register form | `reason`: `invalid`, `captcha`, `rate-limited`, `server` |
| `email-verified` | an email address is verified, by code or by link | `via`: `code`, `link` |
| `signin-succeeded` | a password sign-in succeeds | none |
| `signin-rejected` | a password sign-in is refused | `reason`: `credentials`, `unverified`, `captcha`, `rate-limited`, `server` |
| `oauth-started` | a sign-in or link with an external provider is started | `provider`: `google` |
| `password-reset-requested` | a reset email is requested | none |
| `password-reset-completed` | a new password is set from a reset | none |
| `account-deletion-requested` | a Member schedules deletion of their account | none |

#### Scenario: Sign-up funnel
- **WHEN** a Visitor opens Register, submits the form successfully and verifies their email with a code
- **THEN** the report shows, in order, a `/register` page view, `signup-submitted` and `email-verified` with `via` `code`

#### Scenario: Rejected sign-up
- **WHEN** a Visitor submits the Register form with an email address that fails validation on the server
- **THEN** a `signup-rejected` event with `reason` `invalid` is recorded
- **AND** it carries neither the email address nor the server's error message

#### Scenario: Starting Google sign-in
- **WHEN** a Visitor chooses "Continue with Google" on the Login page
- **THEN** an `oauth-started` event with `provider` `google` is recorded before the browser leaves the site

#### Scenario: Server down during sign-in
- **WHEN** a Visitor submits the Login form and the server cannot be reached
- **THEN** a `signin-rejected` event with `reason` `server` is recorded and the form shows its usual error

### Requirement: Browsers that opt out are not measured
When the browser signals Do Not Track or Global Privacy Control, the site
SHALL record no page views or events for it and SHALL make no analytics
requests.

#### Scenario: Global Privacy Control
- **WHEN** a Visitor whose browser sends Global Privacy Control signs up
- **THEN** no page view or event is recorded and no analytics request leaves the browser

#### Scenario: Do Not Track
- **WHEN** a Member whose browser sends Do Not Track uses the dashboard
- **THEN** no page view or event is recorded

### Requirement: The site works the same whether analytics is on, off or failing
Analytics SHALL be off in an environment unless it is configured there. When it
is off, the site SHALL load no analytics code and make no analytics requests.
When it is on but the analytics service is slow, failing or unreachable, or a
browser extension blocks it, every page and form SHALL behave exactly as with
analytics off: no visible error, no delay to page display or form submission,
and no change to the site's script policy violations (none).

#### Scenario: Analytics not configured
- **WHEN** the site runs with analytics not configured
- **THEN** no page loads analytics code or sends an analytics request

#### Scenario: Analytics service down
- **WHEN** the analytics service is unreachable and a Visitor registers
- **THEN** registration completes as normal with no visible error and no delay

#### Scenario: Analytics blocked by the browser
- **WHEN** an ad blocker blocks the analytics code and a Visitor signs in with Google
- **THEN** sign-in completes as normal

#### Scenario: No script policy violations
- **WHEN** a Visitor opens every public and dashboard page with analytics on
- **THEN** the browser reports no content security policy violations

### Requirement: Analytics traffic stays on the site's own address
Analytics code and data SHALL be served from and sent to the site's own
address. Turning analytics on SHALL NOT allow any new third-party origin in the
site's script policy, and SHALL NOT change the behaviour of any existing page
or API route.

#### Scenario: Only same-site requests
- **WHEN** a Visitor opens the Login page with analytics on
- **THEN** every analytics request goes to the site's own address

#### Scenario: API routes are unaffected
- **WHEN** analytics is on and a Member uses the dashboard
- **THEN** every API request reaches the backend and returns the same response as with analytics off

### Requirement: Only the operator can see analytics reports
Analytics reports SHALL be reachable only by the operator. The public site
SHALL expose only what browsers need to record page views and events. The
recording endpoint SHALL accept only the configured site, so data cannot be
read back through it.

#### Scenario: Report pages are not public
- **WHEN** a Visitor or Member requests the analytics service's report or sign-in pages through the site's address
- **THEN** they receive a not-found response

#### Scenario: Operator views reports
- **WHEN** the operator connects through the operator-only access path and signs in to the analytics service
- **THEN** they can see page views, referrers, events and funnels for the last 90 days

#### Scenario: Recording endpoint rejects other sites
- **WHEN** a request to the recording endpoint names a site that is not configured
- **THEN** nothing is recorded

### Requirement: Analytics data is kept for 90 days
Page views and events SHALL be deleted automatically once they are more than 90
days old. Analytics data SHALL be stored separately from Members' account data
and SHALL NOT be included in account backups.

#### Scenario: Old data is removed
- **WHEN** the daily retention run has completed
- **THEN** no page view or event older than 90 days remains

#### Scenario: Retention run fails
- **WHEN** no retention run has succeeded for 2 days and 15 minutes
- **THEN** the operator is alerted by email naming the analytics retention job, as for the other operational alerts

#### Scenario: Analytics storage is separate
- **WHEN** the analytics storage is lost or wiped
- **THEN** Members' accounts and data are unaffected and the site keeps working
