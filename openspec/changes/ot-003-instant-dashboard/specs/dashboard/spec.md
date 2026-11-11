# Spec Delta

## Purpose

Defines the signed-in area of the app: the header and navigation shared by its
pages, how those pages appear and how a Member moves between them, and how the
dashboard learns and keeps up to date who is signed in.

## ADDED Requirements

### Requirement: The dashboard appears without waiting for the account check
When a browser that holds a session opens a dashboard page, the dashboard SHALL
show the page at once, without a full-screen loading indicator, and SHALL
confirm the signed-in account with the server in the background. Parts of the
page that show account details SHALL use the account details remembered in the
browser tab, or a placeholder when none are remembered, until the check
answers. Every request a page makes SHALL still be authorised by the server.

#### Scenario: Returning Member sees the page at once
- **WHEN** a Member who used the dashboard earlier in this tab reloads the Settings page
- **THEN** the header and Settings content, with the Member's email, are shown before the account check answers
- **AND** no full-screen loading indicator appears

#### Scenario: New tab shows placeholders for account details
- **WHEN** a Member with a session opens the dashboard in a new tab
- **THEN** the header and page layout are shown at once, account-specific details show placeholders
- **AND** the placeholders are replaced with the Member's details when the account check answers

#### Scenario: Remembered details are corrected by the check
- **WHEN** the account details remembered in the tab differ from what the server returns (for example a password was set from another tab)
- **THEN** the dashboard updates to show the server's details without a reload

#### Scenario: The site root opens the dashboard
- **WHEN** a Member with a session opens the site root
- **THEN** they land on the dashboard Home page without the sign-in page being shown on the way

#### Scenario: Visitor at the site root
- **WHEN** a Visitor whose browser has never signed in opens the site root
- **THEN** they land on the sign-in page without any dashboard content being shown

#### Scenario: Visitor never sees the dashboard
- **WHEN** a Visitor whose browser has never signed in, or has signed out, opens any dashboard page directly
- **THEN** they are sent to the sign-in page before any dashboard content is shown

#### Scenario: Session has expired
- **WHEN** a Member's browser holds a session that the server no longer accepts and they open a dashboard page
- **THEN** the dashboard may be shown briefly, then they are sent to the sign-in page with the session-expired notice
- **AND** no data from the server is shown, because the server refused every request

#### Scenario: Unverified email
- **WHEN** a Member whose email is not verified opens a dashboard page
- **THEN** they are sent to the email verification page once the account check answers

#### Scenario: Server unavailable during the check
- **WHEN** a Member opens a dashboard page and the account check cannot reach the server
- **THEN** the Member stays on the page with any remembered details, and sees a notice that the dashboard could not reach the server, with a retry option
- **AND** choosing retry repeats the check, and the notice disappears when it succeeds

### Requirement: The dashboard header stays in place between dashboard pages
When a Member moves from one dashboard page to another using the dashboard
navigation or the browser's back and forward buttons, the dashboard SHALL keep
its header and navigation on screen without rebuilding them, SHALL replace only
the page content, and SHALL NOT show a full-screen loading indicator.

#### Scenario: Moving between dashboard pages keeps the header
- **WHEN** a Member on the dashboard Home page selects Settings in the navigation
- **THEN** the Settings content is shown
- **AND** the header and navigation were not removed or redrawn during the move
- **AND** no full-screen loading indicator appeared

#### Scenario: The current page is highlighted after a move
- **WHEN** a Member moves from Home to Settings
- **THEN** the navigation marks Settings as the current page and no longer marks Home

#### Scenario: Back and forward keep the header
- **WHEN** a Member moves from Home to Settings and then presses the browser's back button
- **THEN** the Home content is shown, Home is marked as the current page, and the header was not rebuilt

#### Scenario: Header stays on a small screen
- **WHEN** a Member on a phone-width screen moves between dashboard pages
- **THEN** the compact navigation stays in place and marks the new current page

#### Scenario: Several moves, one account check
- **WHEN** a Member opens the dashboard and then moves Home → Settings → Home
- **THEN** the server was asked who is signed in at most once

### Requirement: One up-to-date record of the signed-in account
The dashboard SHALL keep a single record of the signed-in account for the
browser tab. A change a Member makes to their own account on one dashboard page
SHALL be reflected on every part of the dashboard in that tab without
reloading, including after moving to another dashboard page.

#### Scenario: Setting a password is remembered across pages
- **WHEN** a Member without a password sets one on Settings, moves to Home, and returns to Settings
- **THEN** Settings offers "Change password", not "Set a password"

#### Scenario: Unlinking Google is remembered across pages
- **WHEN** a Member unlinks Google on Settings, moves to Home, and returns to Settings
- **THEN** Settings shows Google as not linked

#### Scenario: Scheduled deletion is remembered across pages
- **WHEN** a Member schedules account deletion on Settings, moves to Home, and returns to Settings
- **THEN** Settings shows the scheduled deletion date and the option to cancel it

#### Scenario: A failed change leaves the record unchanged
- **WHEN** a Member tries to unlink Google and the server refuses or cannot be reached
- **THEN** an error is shown and Settings, on this and later visits in the tab, still shows Google as linked

#### Scenario: Other tabs are not updated live
- **WHEN** a Member changes their account in one tab while a dashboard page is open in another tab
- **THEN** the other tab shows the change after its next load, and the server's view is authoritative in both

### Requirement: Signing out is prompt and complete
When a Member signs out from the dashboard, the dashboard SHALL discard its
record of the signed-in account and the browser's session hint at once, SHALL
ask the server to end the session, and SHALL show the sign-in page within one
second even if the server is slow or cannot be reached.

#### Scenario: Sign out
- **WHEN** a Member selects Log out on Settings
- **THEN** they are shown the sign-in page and the server is asked to end the session

#### Scenario: Sign out then use the back button
- **WHEN** a Member signs out and then presses the browser's back button
- **THEN** no dashboard content for that account is shown, and they end up on the sign-in page

#### Scenario: Sign out while the server is unavailable
- **WHEN** a Member signs out and the server cannot be reached or does not answer
- **THEN** they are shown the sign-in page within one second, and opening a dashboard page afterwards sends them to sign-in

### Requirement: Admin-only pages stay limited to Admins
The Admin navigation entry SHALL be shown only when the account is known to be
an Admin or a Super Admin. The Admin page SHALL never show the user list to an
account that is not an Admin, and SHALL send such an account to the dashboard
Home page.

#### Scenario: Member does not see the Admin entry
- **WHEN** a Member who is not an Admin opens the dashboard or moves between its pages, including before the account check answers
- **THEN** the navigation never shows the Admin entry

#### Scenario: Member opens the Admin page directly
- **WHEN** a Member who is not an Admin opens the Admin page's address directly
- **THEN** no user list is shown, and they are sent to the dashboard Home page

#### Scenario: Admin moves to the Admin page
- **WHEN** an Admin moves from Home to Admin using the navigation
- **THEN** the user list is shown, Admin is marked as the current page, and the header was not rebuilt
