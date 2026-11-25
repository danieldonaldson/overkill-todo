# Spec Delta

## MODIFIED Requirements

### Requirement: A crashing panel is contained
When a dashboard panel (Home, Lists, Settings, Admin) or the dashboard header fails
with a script error while it is shown or updated, the dashboard SHALL replace
only that part with a short message and a "Try again" action, and SHALL keep
the rest of the dashboard (header, navigation, other pages) working. "Try again"
SHALL rebuild that part without reloading the page. The message SHALL NOT show
the error's text or stack. The same SHALL apply to the forms on the sign-in,
register, verification, password reset and set-password pages.

#### Scenario: Settings panel crashes
- **WHEN** the Settings panel throws a script error while it is shown
- **THEN** the panel area shows a short failure message with "Try again"
- **AND** the header and navigation still work, and moving to Home shows Home normally

#### Scenario: Try again succeeds
- **WHEN** a Member chooses "Try again" after a panel failed because of a temporary problem
- **THEN** the panel is shown again without a page reload

#### Scenario: Try again fails again
- **WHEN** a Member chooses "Try again" and the panel fails again
- **THEN** the failure message is shown again and the page stays usable

#### Scenario: Header crashes
- **WHEN** the dashboard header throws a script error
- **THEN** the page content stays visible and usable, and the header area shows the failure message with "Try again"

#### Scenario: Sign-in form crashes
- **WHEN** the sign-in form throws a script error while a Visitor is on the Login page
- **THEN** the form area shows the failure message with "Try again" and a link to reload the page

#### Scenario: No error details shown
- **WHEN** any panel or form shows the failure message
- **THEN** the message contains no error text, stack or trace ID

#### Scenario: Mobile
- **WHEN** a panel fails on a phone-sized screen
- **THEN** the failure message and "Try again" fit the panel area without horizontal scrolling

#### Scenario: List panel crashes
- **WHEN** the panel showing a list throws a script error
- **THEN** its area shows the failure message with "Try again", and the header and navigation still work

## ADDED Requirements

### Requirement: Lists are part of the dashboard navigation
The dashboard navigation SHALL include Lists. A list page SHALL show the
Member's lists in a picker, marking the open one, next to its todos. On a
phone-width screen the picker SHALL be a drawer. Moving between lists SHALL
keep the dashboard header in place, as for other dashboard pages.

#### Scenario: Open Lists from the navigation
- **WHEN** a Member selects Lists in the navigation
- **THEN** their lists, or the empty state, are shown, Lists is marked as the current page, and the header was not rebuilt

#### Scenario: Move between lists
- **WHEN** a Member selects another list and then presses the browser's back button
- **THEN** each list is shown in turn at its own address, with its entry marked in the picker, and the header was not rebuilt

#### Scenario: Slow answers arrive out of order
- **WHEN** a Member opens one list and, before it has loaded, opens another
- **THEN** the page shows the second list, whichever answer arrives last

#### Scenario: Phone
- **WHEN** a Member on a phone-width screen opens a list and then its picker
- **THEN** the picker opens as a drawer, closes when a list is chosen, and nothing needs horizontal scrolling

#### Scenario: Touch
- **WHEN** a Member on a touch screen scrolls a long list by swiping over its todos
- **THEN** the list scrolls and no todo is moved by accident

#### Scenario: Visitor opens a list address
- **WHEN** a Visitor whose browser has never signed in opens a list's address
- **THEN** they are sent to the sign-in page before any dashboard content is shown

### Requirement: List pages load without blocking the dashboard
A list page SHALL show the header and page layout at once, with placeholders
until its lists and todos load. If they can't be loaded, the page SHALL show a
failure message with a retry option and SHALL NOT show an empty state or "list
not found".

#### Scenario: Slow server
- **WHEN** a Member opens a list page and the server is slow
- **THEN** the header and layout appear at once, with placeholders for the lists and todos

#### Scenario: Server or database unavailable
- **WHEN** a Member opens a list page and the server can't be reached or can't read its data
- **THEN** the page shows a failure message with retry, and retrying after recovery shows the list

#### Scenario: Lists load but todos fail
- **WHEN** the Member's lists load but the open list's todos don't
- **THEN** the picker is usable and only the todos area shows the failure message

#### Scenario: Another account signs in
- **WHEN** a Member signs out and another Member signs in, in the same tab
- **THEN** no list or todo of the first Member is shown at any point

### Requirement: List changes appear at once and save in the background
Every change to a list or todo, except a list deletion awaiting the server's
answer, SHALL appear on screen before the server answers. Changes made in one
tab SHALL be saved in the order they were made and SHALL NOT conflict with each
other. A failed save SHALL be undone on screen with a short message, keeping
other saved or pending changes and any text the Member typed. When it is not
known whether a save took effect, the dashboard SHALL show what the server has
once it can be reached.

#### Scenario: Change appears at once
- **WHEN** a Member marks a todo done while the server takes 3 seconds to answer
- **THEN** it is shown done at once and stays done after the answer

#### Scenario: Rapid edits in one tab
- **WHEN** a Member marks a todo done, not done and done again, and renames it, before the first save is answered
- **THEN** the todo ends done with the new title, on screen and after a reload, with no note that it changed elsewhere

#### Scenario: Edit a todo that is still being added
- **WHEN** a Member adds a todo and renames, completes or deletes it before the addition is answered
- **THEN** both changes take effect

#### Scenario: Save fails
- **WHEN** a Member renames a todo and the server answers with an error
- **THEN** the old title is shown again with a short message, and the typed title can be submitted again

#### Scenario: Addition fails
- **WHEN** a Member adds a todo and it can't be saved
- **THEN** it is removed from the list with a short message and its title can be submitted again

#### Scenario: One of several saves fails
- **WHEN** a Member changes three different todos and the second save fails
- **THEN** only the second change is undone

#### Scenario: An earlier change to the same todo fails
- **WHEN** a Member makes two changes to one todo and the first can't be saved
- **THEN** the todo ends up as the server has it, with one message, and no change is shown that was not saved

#### Scenario: Move fails
- **WHEN** a Member moves a todo and the move can't be saved
- **THEN** the order returns to the server's, with a short message

#### Scenario: Unknown outcome
- **WHEN** the connection drops after a rename is sent and later returns
- **THEN** the page shows the title the server has

#### Scenario: Signed out part-way
- **WHEN** a Member's session is no longer accepted while a change is being saved
- **THEN** they are sent to the sign-in page with the session-expired notice, and the change is not shown as saved

#### Scenario: Moving to another dashboard page while saving
- **WHEN** a Member changes a todo and moves to Settings before it is saved
- **THEN** the save continues, and if it fails they see a short message

#### Scenario: Closing the tab while saving
- **WHEN** a Member closes or reloads the tab while a change is unsaved
- **THEN** the browser asks them to confirm leaving, where the browser supports it

#### Scenario: Messages are announced
- **WHEN** a save fails or a todo changed or was deleted elsewhere
- **THEN** the message is announced to assistive technology without moving focus
