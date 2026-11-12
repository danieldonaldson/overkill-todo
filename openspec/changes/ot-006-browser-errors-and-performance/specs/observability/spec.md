# Spec Delta

## ADDED Requirements

### Requirement: Browser errors are reported
The site SHALL report to the operator every uncaught script error, every
unhandled promise rejection, and every error caught by a panel's error boundary,
on every page, for Visitors and Members alike. Each report SHALL show, in the
operator's log search: the page (as a page template such as `/dashboard/settings`,
never the full address), the release of the site the browser was running, the
error's type and message, its stack, a fingerprint that groups repeats of the
same error, the browser family and device class, and the time. A report from a
signed-in Member SHALL carry that Member's user ID, taken from their session by
the server. A report from a Visitor, or with a missing, expired or invalid
session, SHALL carry no user ID. The server SHALL never take a user ID from the
report's content.

#### Scenario: Uncaught error on a public page
- **WHEN** a script error is thrown and not caught while a Visitor is on the Login page
- **THEN** within 1 minute the operator's log search shows one browser error for page `/login` with the error's message, stack, release and fingerprint
- **AND** the report carries no user ID

#### Scenario: Error from a signed-in Member
- **WHEN** an unhandled promise rejection happens while a Member is on the dashboard Settings page
- **THEN** the report shows page `/dashboard/settings` and that Member's user ID

#### Scenario: Expired session
- **WHEN** a Member's access token has expired and a browser error is reported before the dashboard refreshes the session
- **THEN** the report is recorded with no user ID
- **AND** sending it causes no session refresh, sign-out or redirect

#### Scenario: User ID in the report body is ignored
- **WHEN** a client sends a report whose content claims a user ID
- **THEN** the recorded report carries the user ID from the session, or none, and never the claimed one

#### Scenario: Unknown page
- **WHEN** a browser error happens on an address that is not one of the site's pages
- **THEN** it is recorded under the page `other`

#### Scenario: The same error repeats
- **WHEN** the same error is thrown 50 times on one page load
- **THEN** it is reported once, with a count of 50

#### Scenario: An error loop on one page
- **WHEN** a page throws 500 different errors in one page load
- **THEN** at most 20 browser error reports are sent from that page load

#### Scenario: Events from different page loads are grouped
- **WHEN** the operator looks at two browser errors from the same page load
- **THEN** both carry the same page-load ID, and errors from any other page load carry a different one

#### Scenario: Signing in or out starts a new page-load ID
- **WHEN** a Visitor signs in, or a Member signs out, without the browser loading a new page
- **THEN** browser errors from before and after carry different page-load IDs

### Requirement: Failed API calls in the browser link to the backend trace
When an API call made by the site fails with a server error (5xx) or cannot reach
the server, the site SHALL report it as a browser error that records the API
route template, the status (or that the network failed), and, when the server
answered, the response's trace ID. From that report the operator SHALL be able
to open the backend trace in one click. Client errors (4xx) SHALL NOT be reported
as browser errors.

#### Scenario: Server error during a dashboard action
- **WHEN** a Member saves their name and the server answers with a 500
- **THEN** the browser error report records the route, status 500 and the response's trace ID
- **AND** one click from the report opens that trace

#### Scenario: Server unreachable
- **WHEN** a Visitor submits the Register form and the server cannot be reached
- **THEN** a browser error is recorded for the route with a network failure and no trace ID, once the server is reachable again and the page is still open or is next hidden

#### Scenario: Validation errors are not browser errors
- **WHEN** a Visitor submits the Login form with a wrong password and the server answers 401
- **THEN** no browser error is reported

### Requirement: Real-user performance is measured
For a configurable share of page loads (all of them unless configured lower), the
site SHALL measure in the browser and report Largest Contentful Paint, First
Contentful Paint and Time to First Byte for each full page load, and
Interaction to Next Paint and Cumulative Layout Shift for each page visit,
including visits reached by in-site navigation. The operator SHALL be able to see the 75th percentile of
each, per page template, release, device class (mobile, tablet, desktop) and
signed-in or not, for a selectable time range up to 30 days, and the share of
measurements rated good, needs improvement and poor. The site SHALL also count
page loads per page template, so error rates can be shown per page load.

#### Scenario: Dashboard shows Web Vitals per page
- **WHEN** Members have loaded the dashboard Home page on phones and desktops
- **THEN** the Frontend dashboard shows the 75th percentile LCP, INP and CLS for `/dashboard`, separately for mobile and desktop

#### Scenario: Compare releases
- **WHEN** a new release of the site goes out
- **THEN** the operator can compare the Web Vitals and browser error rate of the new release with the previous one

#### Scenario: Sampling
- **WHEN** the share is configured as 10%
- **THEN** about 10% of page loads report Web Vitals, and every page load still reports browser errors

#### Scenario: Navigating inside the dashboard
- **WHEN** a Member loads Home, moves to Settings using the dashboard navigation, and interacts with Settings
- **THEN** page loads are counted for both pages
- **AND** INP and CLS from the Settings visit are reported for `/dashboard/settings`, not for `/dashboard`
- **AND** LCP, FCP and TTFB are reported only for `/dashboard`, the full page load

#### Scenario: Leaving the page on a phone
- **WHEN** a Visitor on a phone switches to another app while on the Register page
- **THEN** the measurements and errors collected so far are sent before the page is hidden

### Requirement: In-site navigations are measured
For the same sampled page loads, the site SHALL measure every in-site navigation
(moving between pages without a full page load, by link or by the browser's back
and forward buttons) and report:
- the total time from the start of the navigation to the new page being usable
  and any transition animation having finished;
- the time spent fetching the new page;
- the time spent swapping it in, including the animation;
- whether an animated view transition ran, was skipped, or is not supported by
  the browser.

A navigation that is interrupted by another before it finishes SHALL be counted
as aborted, with no duration. The operator SHALL be able to see the 75th
percentile of each time per destination page, release and device class, and the
share of navigations whose transition ran, was skipped or was unsupported, and
that were aborted.

#### Scenario: Navigation from Home to Settings
- **WHEN** a Member on Home clicks Settings in the dashboard navigation
- **THEN** one in-site navigation to `/dashboard/settings` is reported with its total, fetch and swap times and whether the view transition ran

#### Scenario: Back button
- **WHEN** a Member on Settings presses the browser's back button and returns to Home without a full page load
- **THEN** one in-site navigation to `/dashboard` is reported

#### Scenario: Browser without view transitions
- **WHEN** a Member navigates in a browser that does not support view transitions
- **THEN** the navigation is still measured and reported with the transition marked unsupported

#### Scenario: Interrupted navigation
- **WHEN** a Member clicks Settings and, before it has finished, clicks Admin
- **THEN** the navigation to Settings is counted as aborted with no duration
- **AND** the navigation to Admin is reported as completed with its times

#### Scenario: Navigation falls back to a full page load
- **WHEN** an in-site navigation cannot be completed in place and the browser loads the new page in full
- **THEN** it is reported as a full page load of the new page, not as an in-site navigation

#### Scenario: Navigating on a phone
- **WHEN** a Member on a phone moves from Home to Settings
- **THEN** the navigation is reported under the mobile device class

### Requirement: Panel loading is measured
For the same sampled page loads, the site SHALL measure, for each interactive
panel and form, the time from the start of the navigation that showed it (the
start of the page load for a full page load, the start of the navigation for an
in-site navigation) to the moment it became interactive and the moment it was
ready (showing its content, its empty state, or its error state). The operator
SHALL be able to see the 75th percentile of each, per panel, kind of navigation
and device class. A panel that stays on screen across in-site navigation SHALL be
measured only when it first appears.

#### Scenario: Home after a full page load
- **WHEN** a Member opens the dashboard Home page with a full page load
- **THEN** the Home panel's times to interactive and to ready are reported for a full page load

#### Scenario: Settings after an in-site navigation
- **WHEN** a Member moves from Home to Settings using the dashboard navigation
- **THEN** the Settings panel's times to interactive and to ready are reported for an in-site navigation, measured from the click

#### Scenario: The dashboard header across navigation
- **WHEN** a Member moves between dashboard pages
- **THEN** the header's loading is reported only for the first full page load, not again for each navigation

#### Scenario: Panel that fails to load its data
- **WHEN** the Admin panel's user list request fails and the panel shows its error state
- **THEN** the panel's time to ready is reported at the moment the error state is shown

#### Scenario: Leaving before a panel is ready
- **WHEN** a Member leaves Settings before its panel has shown its content
- **THEN** no ready time is reported for it, and no error results

#### Scenario: Unknown panel name
- **WHEN** a report names a panel that is not one of the site's panels
- **THEN** it is recorded under the panel `other`

### Requirement: Browser reports never expose secrets or personal data
Browser reports SHALL NOT contain query strings or URL fragments (in the page,
in error messages, in stacks or in file names), email addresses, form field
values, cookies, tokens, codes, or request or response bodies. Browser reports
SHALL NOT contain the client's IP address. The site SHALL store nothing in the
browser (no cookies, no browser storage) for browser reports.

#### Scenario: Email address in the page address is never recorded
- **WHEN** a Visitor is on the password reset page, whose address carries their email address in the query string, and a script error is thrown whose message and stack include the full address
- **THEN** a search of all telemetry finds neither the email address nor the query string

#### Scenario: Magic sign-in token is never recorded
- **WHEN** a Visitor opens a magic sign-in link, whose token is in the address fragment, and a script error is thrown and an API call on that page fails
- **THEN** no telemetry contains the magic-link token or the fragment

#### Scenario: Email address in an error message
- **WHEN** a script error message contains an email address
- **THEN** the stored message shows `[EMAIL]` in its place

#### Scenario: Long message
- **WHEN** a script error has a 20,000-character message
- **THEN** the stored message is cut to at most 500 characters and the stack to at most 4,000

#### Scenario: Nothing stored in the browser
- **WHEN** a Visitor opens any page and a browser error is reported
- **THEN** no cookie or browser storage is written for browser reports

### Requirement: Browser reporting never affects the page
Reporting browser errors and performance SHALL NOT delay page display or any
interaction, SHALL NOT show anything to the Visitor or Member, and SHALL NOT
cause script errors of its own that reach the page. When the server is slow,
failing or unreachable, or the report is blocked by the browser or an extension,
the page SHALL behave exactly as if reporting did not exist. Reporting SHALL NOT
add any new third-party origin to the site's script policy.

#### Scenario: Backend down
- **WHEN** the backend is unreachable and a Member uses the dashboard
- **THEN** pages and panels behave as they do without reporting, with no added error message or delay

#### Scenario: Blocked by an extension
- **WHEN** a content blocker blocks browser reports and a Visitor signs in
- **THEN** sign-in completes as normal

#### Scenario: No script policy violations
- **WHEN** a Visitor opens every public and dashboard page and a browser error is reported on each
- **THEN** the browser reports no content security policy violations

### Requirement: The browser report endpoint is bounded
The server SHALL accept browser reports only in the documented shape and size and
SHALL reject anything else without recording it. Field values SHALL come from
fixed lists where the documented shape says so (page template, measurement name,
rating, device class, panel name, navigation kind, transition result,
navigation phase), so that no input can create unbounded numbers of metric
series. The server SHALL limit the total rate at which it records browser
reports and SHALL count, but not record, reports above that limit. Browser
reports SHALL NOT affect the latency of other API requests beyond the limits
stated for telemetry failures.

#### Scenario: Malformed report
- **WHEN** a client sends a report that is not valid against the documented shape
- **THEN** it receives a 400 with the standard error body and nothing is recorded

#### Scenario: Oversized report
- **WHEN** a client sends a report larger than 16 KB
- **THEN** it receives a 413 and nothing is recorded

#### Scenario: Unknown metric values cannot create series
- **WHEN** a client sends thousands of reports with made-up page names, panel names and measurement values
- **THEN** made-up page and panel names are recorded as `other`, unknown measurement names, navigation kinds, transition results and phases are rejected, and the number of metric series does not grow

#### Scenario: Flood of reports
- **WHEN** clients send browser reports faster than the server's recording limit
- **THEN** reports above the limit are answered as accepted, not recorded, and counted as dropped
- **AND** sign-in and dashboard requests keep their normal latency

### Requirement: Operator sees and is alerted to browser problems
The operator SHALL have a Frontend dashboard showing: browser error rate (reports
per page load) per page template and release; the most frequent error
fingerprints with first and last seen times; the Web Vitals, in-site navigation
and panel loading measurements described above; and dropped browser reports. From an error on the dashboard the operator SHALL
reach its example reports and, where present, the backend trace. The system SHALL
email the operator, and send a resolved notice when it clears, when:

- browser errors exceed 2% of page loads for 10 minutes (only counted when there is at least 1 page load per minute)
- the 75th percentile Interaction to Next Paint is above 500 ms, or the 75th percentile Largest Contentful Paint is above 4 seconds, on any page template for 1 hour (only counted when that page has at least 50 measurements in the hour)
- the 75th percentile total time of in-site navigations to any dashboard page is above 1 second for 1 hour (only counted when that page has at least 50 completed navigations in the hour)

#### Scenario: Error spike after a release
- **WHEN** a new release makes 5% of dashboard page loads throw a script error for 10 minutes
- **THEN** the operator receives one email naming the alert, the page, the release and a link to the Frontend dashboard
- **AND** receives a resolved email once the rate stays below the threshold

#### Scenario: No alert on low traffic
- **WHEN** 1 of 3 page loads in a quiet 10-minute window reports a browser error
- **THEN** no browser error alert is sent

#### Scenario: Slow page alert
- **WHEN** the 75th percentile INP on `/dashboard/settings` stays above 500 ms for an hour with 200 measurements
- **THEN** the operator receives a poor-responsiveness alert naming the page

#### Scenario: Slow navigation alert
- **WHEN** the 75th percentile time of in-site navigations to `/dashboard/admin` stays above 1 second for an hour with 120 completed navigations
- **THEN** the operator receives a slow-navigation alert naming the page

#### Scenario: Decode a minified stack
- **WHEN** the operator has a browser error's stack and release from a production build
- **THEN** they can turn it into original source file names and line numbers without the source maps ever having been served by the site

#### Scenario: Source maps are not public
- **WHEN** a Visitor requests a source map file for any of the site's scripts
- **THEN** the site answers 404

## MODIFIED Requirements

### Requirement: Every request is traceable
The system SHALL record a trace for every HTTP request handled by the backend,
except health checks and browser reports.
The trace SHALL start at the edge and include the backend's handling of the
request and each database call made for it. Every backend response SHALL carry
an `X-Trace-Id` header containing that trace's 32-character lowercase hex ID.

#### Scenario: Successful request produces a full trace
- **WHEN** a Member signs in successfully
- **THEN** the response carries an `X-Trace-Id` header of 32 lowercase hex characters
- **AND** within 1 minute the operator can open that trace in the operations dashboard
- **AND** the trace shows the edge, the backend request and at least one database call as nested steps with their durations

#### Scenario: Failed request is traced and marked as an error
- **WHEN** a request fails with a 5xx response
- **THEN** its trace is recorded and marked as an error
- **AND** the trace shows the route template (for example `/api/v1/auth/oauth/{provider}`), not the raw path

#### Scenario: Client-supplied trace context is ignored
- **WHEN** a Visitor sends a request with their own `traceparent` header
- **THEN** the system starts a new trace and does not adopt the supplied trace ID

#### Scenario: Health checks do not flood traces
- **WHEN** the liveness and readiness endpoints are polled
- **THEN** no traces are recorded for those requests

#### Scenario: Browser reports do not flood traces
- **WHEN** browsers send browser error and performance reports
- **THEN** no traces are recorded for those requests, and they are still counted in the per-route request metrics

### Requirement: Observability tools are restricted to the operator
The operations dashboard, alert views and every telemetry ingestion or query
endpoint SHALL NOT be reachable from the public internet. The dashboard SHALL
require the operator to sign in. App roles (Member, Admin, Super Admin) grant no
access to it. The site's browser report endpoint is an app endpoint, not a
telemetry ingestion endpoint: it is public, accepts only the documented report
shape, and gives no way to read telemetry back.

#### Scenario: Visitor cannot reach telemetry
- **WHEN** a Visitor requests the dashboard, a metrics endpoint or a telemetry ingestion endpoint on the public domain
- **THEN** the request does not reach any of them (the edge returns 404, or the port is closed)

#### Scenario: Admin of the app has no dashboard access
- **WHEN** an Admin is signed in to the app
- **THEN** that session gives no access to the operations dashboard

#### Scenario: Operator signs in to the dashboard
- **WHEN** the operator connects through the private access path and signs in with the dashboard credentials
- **THEN** the dashboards, log search, trace search and alert views are available

#### Scenario: Browser report endpoint cannot read telemetry
- **WHEN** a Visitor sends any request to the browser report endpoint
- **THEN** the response body is empty or the standard error body, and never contains stored telemetry
