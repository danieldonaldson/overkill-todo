# Spec Delta

## ADDED Requirements

### Requirement: A tab never saves to an account it doesn't show

Every change made in a dashboard tab SHALL be sent for the account that tab
showed when the Member acted, including when it is sent again after the session
was refreshed. When it is refused because another account is signed in, the tab
SHALL save nothing, SHALL NOT send it again, SHALL remove the previous account's
data and unsaved text, SHALL show the signed-in account, and SHALL say once that
nothing was saved.

#### Scenario: Another Member signed in from a second tab

- **WHEN** Member A's tab is open, Member B signs in from a second tab, and a list is created in the first tab
- **THEN** B's account gets no list, the first tab shows B's lists, and one message says nothing was saved

#### Scenario: The account changes while a save is on its way

- **WHEN** a change is made as A, its session has to be refreshed, and B signs in before it is sent again
- **THEN** it is refused and B's account is unchanged

#### Scenario: Several unsaved changes

- **WHEN** three list changes are waiting as the account changes
- **THEN** none is saved and one message is shown

#### Scenario: Settings change

- **WHEN** a Member changes the password, unlinks Google, or asks for or cancels deletion in a tab showing another account than the one signed in
- **THEN** nothing changes on either account and the tab shows the signed-in account

#### Scenario: Admin change

- **WHEN** an Admin page is open and a Member without that role signs in from another tab, and a role is then changed on the Admin page
- **THEN** nothing changes and the Admin page is no longer shown

#### Scenario: Account check unavailable

- **WHEN** a change is refused this way and the account can't then be loaded
- **THEN** the previous account's data is gone and the existing "can't reach the server" notice with retry is shown

#### Scenario: Signed out elsewhere

- **WHEN** the session ended in another tab and nobody has signed in
- **THEN** the Member is sent to sign in, as today

#### Scenario: Page opened before the release

- **WHEN** a tab opened before this was released makes a change
- **THEN** it is saved as before
