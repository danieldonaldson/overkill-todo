# Spec Delta

## ADDED Requirements

### Requirement: A crashing panel is contained
When a dashboard panel (Home, Settings, Admin) or the dashboard header fails
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
