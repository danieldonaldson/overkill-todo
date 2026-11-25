# Spec Delta

## MODIFIED Requirements

### Requirement: Telemetry never exposes secrets or personal data
Telemetry (traces, logs and metrics) SHALL NOT contain passwords, password
hashes, access or refresh tokens, verification or reset codes, magic-link
tokens, OAuth codes or state, CSRF tokens, cookies, `Authorization` headers,
session identifiers, request and response bodies of auth, list and todo routes,
list names or todo titles. Email
addresses SHALL NOT appear in traces or backend logs; a Member is identified by
user ID. Client IP addresses SHALL appear only in edge access logs.

#### Scenario: Sign-in leaves no secrets behind
- **WHEN** a Member signs in with email and password and refreshes their session
- **THEN** a search of all telemetry from that window finds neither the password, the email address, the access token, the refresh token nor the CSRF token

#### Scenario: Registration and reset flows leave no secrets behind
- **WHEN** a Visitor registers, verifies their email with a code, and later resets their password with a reset code
- **THEN** no telemetry contains the email address, either code, the magic-link token or the new password

#### Scenario: Sensitive query parameters are stripped
- **WHEN** a request URL carries a query parameter named `token`, `code`, `password`, `secret`, `key` or `state`
- **THEN** the recorded URL in traces and logs shows the parameter's value replaced with `[REDACTED]`

#### Scenario: A sensitive field added by mistake is still scrubbed
- **WHEN** a backend log line or trace attribute is written with a field whose name contains `password`, `token`, `secret`, `code`, `otp`, `session` or `cookie`
- **THEN** the stored value is `[REDACTED]`

#### Scenario: Upstream error bodies are not stored
- **WHEN** a call to an external identity provider fails
- **THEN** the log line records the status and error kind but not the provider's response body

#### Scenario: Todos leave no content behind
- **WHEN** a Member creates and renames a list and adds, renames, completes, moves and deletes a todo
- **THEN** a search of all telemetry from that window finds neither the list names nor the todo titles

#### Scenario: Refused or failed saves leave no content behind
- **WHEN** a list name or todo title is refused as invalid, conflicts with a change made elsewhere, or can't be saved because the database is unavailable
- **THEN** no telemetry contains the name or title

### Requirement: Browser reports never expose secrets or personal data
Browser reports SHALL NOT contain query strings or URL fragments (in the page,
in error messages, in stacks or in file names), list IDs taken from a page's
address, list names, todo titles, email addresses, form field
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

#### Scenario: Script error on a list page
- **WHEN** a script error whose message and stack include the list page's full address is reported
- **THEN** a search of all browser reports finds no list ID

#### Scenario: Content in a script error
- **WHEN** adding, renaming or showing a todo fails with a script error
- **THEN** no browser report contains the list name or the todo title

## ADDED Requirements

### Requirement: List pages are measured as one page
Browser errors, page loads, real-user performance, in-site navigations and
panel loading on list pages SHALL be recorded under the one page template
`/dashboard/lists/[id]`, whichever list is open. Moving between lists SHALL
count as an in-site navigation to that page.

#### Scenario: Several lists, one page
- **WHEN** Members open thousands of different lists
- **THEN** all measurements are under `/dashboard/lists/[id]` and the number of metric series does not grow with the number of lists

#### Scenario: Moving between lists
- **WHEN** a Member on one list selects another in the list picker
- **THEN** one in-site navigation to `/dashboard/lists/[id]` is reported

#### Scenario: Error on a list page
- **WHEN** a script error happens on a list page
- **THEN** it is recorded under the page `/dashboard/lists/[id]`, not `other`

#### Scenario: Backend routes
- **WHEN** Members send requests for many lists and todos
- **THEN** request metrics are recorded per route template, not per list or todo
