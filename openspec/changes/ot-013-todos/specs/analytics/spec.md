# Spec Delta

## MODIFIED Requirements

### Requirement: Page views are recorded anonymously
When analytics is on, the site SHALL record a page view for every page a
Visitor or Member opens, including moves between dashboard pages that don't
reload the page. A page view SHALL record only the page's path (for a list page,
always the one path `/dashboard/lists/[id]`, never the list's own address), the referring
site, and coarse context derived by the analytics service (approximate location down to
city, browser, operating system, device type, screen size, language). It SHALL NOT record the
query string, the URL fragment, the page title of a list page, a list name or a
todo title.

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

#### Scenario: List pages are one page
- **WHEN** a Member opens three different lists, by full page load or in-site navigation
- **THEN** the report shows three page views of `/dashboard/lists/[id]`
- **AND** a search of all stored analytics data finds no list ID, list name or todo title

#### Scenario: Leaving a list page
- **WHEN** a Member moves from a list page to Settings
- **THEN** the page view of `/dashboard/settings` carries no list ID as its referrer

#### Scenario: Address that is not a list
- **WHEN** a Member opens a list address that shows "list not found"
- **THEN** it is recorded as `/dashboard/lists/[id]`
