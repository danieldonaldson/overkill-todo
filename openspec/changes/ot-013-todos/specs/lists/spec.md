# Spec Delta

## Purpose

Lets a Member keep todos in lists they create: organising, ordering and
completing them, with limits, safe concurrent edits, privacy, export and
erasure.

## ADDED Requirements

### Requirement: A Member manages their lists
A Member with a verified email address SHALL be able to create, rename and
delete lists. Lists SHALL be shown oldest first, each with its count of open
todos. Two lists MAY have the same name. A new Member SHALL have no lists.

#### Scenario: No lists yet
- **WHEN** a Member with no lists opens the dashboard's lists
- **THEN** they see an empty state inviting them to create a list

#### Scenario: Create a list
- **WHEN** a Member creates a list named "Work"
- **THEN** "Work" appears last among their lists with 0 open todos, and is still there after a reload

#### Scenario: Rename a list
- **WHEN** a Member renames "Work" to "Job"
- **THEN** the list is shown as "Job" in the same position, at the same address

#### Scenario: Count of open todos
- **WHEN** a list holds 3 open and 2 done todos
- **THEN** the list is shown with a count of 3

### Requirement: Deleting a list is confirmed and permanent
Deleting a list SHALL require a confirmation that names the list and how many
todos it holds, open and done. Confirming SHALL permanently delete the list and
all its todos. If the list's name or number of todos has changed since the
confirmation was shown, the server SHALL delete nothing, and the Member SHALL
be asked to confirm again with the current name and number.

#### Scenario: Confirm deletion
- **WHEN** a Member deletes a list holding 3 open and 2 done todos
- **THEN** the confirmation names the list and 5 todos
- **AND** after confirming, the list and its todos are gone, also after a reload

#### Scenario: Cancel deletion
- **WHEN** a Member cancels or leaves the page at the confirmation
- **THEN** nothing is deleted

#### Scenario: Todo added in another tab before confirming
- **WHEN** the confirmation names 5 todos, another tab adds a sixth, and the Member confirms
- **THEN** nothing is deleted and the Member is asked to confirm deleting 6 todos

### Requirement: Each list has its own address
Every list SHALL have its own address under the dashboard that opens that list
for its Member. An address whose list was deleted, belongs to another account,
never existed or is malformed SHALL show the same "list not found" message,
with a way back to the Member's lists.

#### Scenario: Reload or bookmark
- **WHEN** a Member reloads a list page or opens its bookmark
- **THEN** that list is shown

#### Scenario: Address does not open a list
- **WHEN** a Member opens the address of a list that was deleted, belongs to another Member, or never existed, or an address with a malformed list ID
- **THEN** each shows the same "list not found" message

#### Scenario: Shared address
- **WHEN** a Member sends a list's address to another Member, who opens it
- **THEN** the other Member sees "list not found"

### Requirement: A Member manages the todos in a list
Within a list, a Member with a verified email address SHALL be able to add
todos, mark them done or not done, rename and delete them. Every todo SHALL
belong to exactly one list. A new todo SHALL go to the bottom of the open
todos. Done todos SHALL be in a "Completed" group, collapsed at first, most
recently completed first. A todo marked not done SHALL go to the bottom of the
open todos. Deleting a todo SHALL be permanent.

#### Scenario: Empty list
- **WHEN** a Member opens a list with no todos
- **THEN** they see an empty state and the field for adding a todo, and no "Completed" group

#### Scenario: Add a todo
- **WHEN** a Member adds "Buy milk" to a list with open todos
- **THEN** it appears at the bottom of the open todos, and the list's count goes up by 1

#### Scenario: Mark done
- **WHEN** a Member marks an open todo done
- **THEN** it moves to the top of "Completed" and the list's count goes down by 1

#### Scenario: Mark not done
- **WHEN** a Member marks a done todo not done
- **THEN** it moves to the bottom of the open todos and the list's count goes up by 1

#### Scenario: Rename a todo
- **WHEN** a Member renames an open or done todo
- **THEN** it shows the new title in the same position

#### Scenario: Delete a todo
- **WHEN** a Member deletes a todo
- **THEN** it is gone, also after a reload

#### Scenario: Abandon a rename
- **WHEN** a Member starts renaming a todo and cancels, or leaves the page before submitting
- **THEN** the title is unchanged

### Requirement: Open todos can be reordered
A Member SHALL be able to move an open todo to any position among the list's
open todos, by dragging (mouse or touch) or with the keyboard alone. The order
SHALL be kept. Keyboard moves SHALL be announced to assistive technology with
the todo's new position. Done todos SHALL NOT be reorderable.

#### Scenario: Drag to a new position
- **WHEN** a Member drags the third open todo to the top
- **THEN** it is first, the others keep their relative order, and the order is the same after a reload

#### Scenario: Move with the keyboard
- **WHEN** a Member moves a todo up one place using only the keyboard
- **THEN** it moves up one place, keeps focus, and its new position is announced

#### Scenario: Cancelled drag
- **WHEN** a Member starts a drag and cancels it
- **THEN** the order is unchanged and nothing is saved

#### Scenario: Many moves
- **WHEN** a Member makes 1,000 moves in one list, including repeatedly between the same two todos
- **THEN** every move succeeds and no todo is lost, duplicated or left out of place

### Requirement: Names and titles are validated
List names SHALL be 1 to 100 characters and todo titles 1 to 500 characters,
counted as Unicode characters after surrounding whitespace is removed. They
SHALL be a single line without control characters. In the dashboard, line
breaks in pasted text SHALL become spaces. Text SHALL be shown exactly as
stored and never interpreted as markup. The server SHALL refuse invalid input
with a 400 and save nothing.

#### Scenario: Blank
- **WHEN** a Member submits a list name or todo title that is empty or only whitespace
- **THEN** nothing is saved and they see a message that it can't be empty

#### Scenario: At the length limit
- **WHEN** a Member saves a 100-character list name and a 500-character todo title that include emoji and accented letters
- **THEN** both are saved unchanged

#### Scenario: Too long
- **WHEN** a Member submits a 101-character list name or a 501-character todo title
- **THEN** nothing is saved, the message names the limit, and their text is kept for editing

#### Scenario: Control characters sent to the server
- **WHEN** a client sends a name or title containing a line break, a null character or another control character
- **THEN** the server responds 400 with the standard error body and nothing is saved

#### Scenario: Pasted lines
- **WHEN** a Member pastes text with line breaks into the add field
- **THEN** it becomes one title with spaces in place of the line breaks

#### Scenario: Markup in a title
- **WHEN** a Member saves the title `<img src=x onerror=alert(1)>`
- **THEN** that text is shown literally and nothing runs

### Requirement: Lists and todos have limits
A Member SHALL have at most 100 lists, and a list at most 1,000 todos, open and
done together. The server SHALL refuse an addition beyond a limit and save
nothing, and the Member SHALL see a message naming the limit. The limits SHALL
hold under concurrent requests.

#### Scenario: List limit
- **WHEN** a Member with 100 lists creates another
- **THEN** nothing is saved and the message says they can have at most 100 lists

#### Scenario: Todo limit
- **WHEN** a Member adds a todo to a list holding 600 open and 400 done todos
- **THEN** nothing is saved, the message says a list can hold at most 1,000 todos, and the typed title is kept

#### Scenario: Room again after deleting
- **WHEN** a Member at a limit deletes one todo or list and adds another
- **THEN** the addition is saved

#### Scenario: Concurrent additions at the limit
- **WHEN** two tabs each add a todo at the same time to a list holding 999 todos
- **THEN** one is saved, the other is refused, and the list holds 1,000

#### Scenario: A full list stays usable
- **WHEN** a Member opens a list holding 1,000 todos
- **THEN** all of them can be reached, and adding, marking done, renaming and reordering work as in a short list

### Requirement: Edits made elsewhere are never silently overwritten
A rename, a change of done state, or a deletion of a todo, and a rename of a
list, SHALL take effect only if that todo's or list's name and done state are
still as the Member last saw them. Otherwise the server SHALL change nothing,
and the dashboard SHALL show the current version with a note that it changed
elsewhere. A change that asks for the state already stored SHALL succeed with
no note. Position SHALL NOT count as a difference, and changing one todo SHALL
NOT conflict with changes to others. A client that does not say which version
it saw SHALL be refused with a 400.

#### Scenario: Renamed in two tabs
- **WHEN** two tabs show the same todo, one renames it to "A", and the other then renames it to "B"
- **THEN** "A" is kept, and the second tab shows "A" with a note that it changed elsewhere
- **AND** the second tab still offers "B" so the Member can submit it again

#### Scenario: Renamed in one tab, marked done in another
- **WHEN** one tab renames a todo and another, still showing the old title, marks it done
- **THEN** the todo keeps the new title and stays open, and the second tab shows it with a note that it changed elsewhere

#### Scenario: Marked done in both tabs
- **WHEN** two tabs both mark the same open todo done
- **THEN** it is done and neither tab shows a note

#### Scenario: Deleting a todo renamed elsewhere
- **WHEN** one tab renames a todo and another, still showing the old title, deletes it
- **THEN** the todo is kept, and the second tab shows the new title with a note that it changed elsewhere

#### Scenario: List renamed in two tabs
- **WHEN** two tabs show the same list, one renames it, and the other then renames it differently
- **THEN** the first name is kept and the second tab shows it with a note that it changed elsewhere

#### Scenario: Different todos in two tabs
- **WHEN** two tabs each rename a different todo in the same list
- **THEN** both renames are saved and neither tab shows a note

#### Scenario: Moved in one tab, renamed in another
- **WHEN** one tab moves a todo and another, not yet showing the move, renames it
- **THEN** both the new position and the new title are kept

#### Scenario: No version given
- **WHEN** a client sends a change to a todo or list without saying which version it saw
- **THEN** the server responds 400 and changes nothing

### Requirement: Changes to something deleted elsewhere are handled
When a Member changes a todo or list that was deleted elsewhere, the dashboard
SHALL remove it from the page with a note that it was deleted elsewhere, and
the server SHALL NOT recreate it. Deleting something already deleted SHALL
simply remove it.

#### Scenario: Todo deleted elsewhere
- **WHEN** a Member renames, marks or moves a todo that another tab has deleted
- **THEN** it is removed from the page with a note that it was deleted elsewhere

#### Scenario: List deleted elsewhere
- **WHEN** a Member adds or changes a todo in a list that another tab has deleted
- **THEN** nothing is saved, the list leaves the list picker, and the page shows "list not found" with a note that it was deleted elsewhere

#### Scenario: Deleted in both tabs
- **WHEN** two tabs both delete the same todo
- **THEN** it is gone from both, with no error

### Requirement: Concurrent moves settle on one order
Concurrent moves, additions, completions and deletions in a list SHALL never
lose or duplicate a todo, and SHALL leave the server with exactly one order.
Each tab SHALL show that order once it has next saved a move in that list or
loaded the list. A move relative to a todo that has been deleted or completed
elsewhere SHALL NOT fail for that reason.

#### Scenario: Different todos moved at once
- **WHEN** two tabs move different todos in the same list at the same time
- **THEN** every todo is present exactly once, and after a reload both tabs show the same order

#### Scenario: Same todo moved at once
- **WHEN** two tabs move the same todo to different positions at the same time
- **THEN** it ends up in one of the two positions, and after a reload both tabs show the same order

#### Scenario: Neighbour gone
- **WHEN** a Member moves a todo next to one that another tab has deleted or marked done
- **THEN** the move is saved at the nearest remaining position and the tab shows the resulting order

#### Scenario: Moved todo was completed elsewhere
- **WHEN** a Member moves a todo that another tab has marked done
- **THEN** it stays done, and the tab shows it under "Completed" with a note that it changed elsewhere

### Requirement: A repeated save takes effect once
Repeating a request to create a list or add a todo because its answer was lost
SHALL NOT create a second one.

#### Scenario: Answer lost
- **WHEN** a Member adds a todo, the server saves it, the answer is lost, and the save is retried
- **THEN** the list holds that todo once

#### Scenario: Same title on purpose
- **WHEN** a Member adds "Buy milk" twice as two separate actions
- **THEN** the list holds two todos titled "Buy milk"

### Requirement: Lists follow the account through export and deletion
A Member's data export SHALL include all their lists and todos, with names,
titles, done state, order, and creation and completion times. Lists and todos
SHALL stay usable while account deletion is pending and SHALL be kept if it is
cancelled. They SHALL be permanently erased when the account is purged.

#### Scenario: Export
- **WHEN** a Member with lists holding open and done todos exports their data
- **THEN** the export contains every one of those lists and todos, in order

#### Scenario: Export with no lists
- **WHEN** a Member with no lists exports their data
- **THEN** the export succeeds with an empty set of lists

#### Scenario: Largest export
- **WHEN** a Member with 100 lists of 1,000 todos exports their data
- **THEN** the export is complete

#### Scenario: Deletion pending, then cancelled
- **WHEN** a Member schedules account deletion, adds a todo, and cancels the deletion
- **THEN** all their lists and todos, including the new one, are still there

#### Scenario: Account purged
- **WHEN** the purge of an account succeeds
- **THEN** none of its lists or todos remain in the live data, and no other Member's are affected

#### Scenario: Purge fails part-way
- **WHEN** erasing an account's lists fails during the purge
- **THEN** the account is not marked deleted, and the next purge run completes both
