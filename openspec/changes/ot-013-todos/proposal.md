# OT-013: Todo lists

## Why

As a Member, I want to keep my todos in lists I create, so that the app does
the one thing its name promises.

The app has finished auth, observability, an audit log, fuzzing and formal
models, but a signed-in Member has nothing to do yet. Todos are the product,
and they're also the base the planned collaboration work builds on: shared
lists, live updates and invite links. So OT-013 is deliberately personal and
small, but it settles the parts that would be expensive to change once lists
are shared. Every todo belongs to a list. Edits from another tab are never
silently overwritten. Manual order holds up under concurrent moves.

## What Changes

- A Member can create, rename and delete lists. Deleting a list asks for
  confirmation (naming how many todos it holds) and is permanent. A new Member
  starts with no lists and sees an empty state inviting them to create one.
  Lists are shown oldest first, each with its count of open todos.
- Each list has its own address under the dashboard. Bookmarking, reloading or
  sharing the address reopens that list. A list that was deleted, belongs to
  someone else or never existed shows the same "list not found".
- Within a list, a Member adds todos (a title, added at the bottom), ticks them
  done or not done, renames and deletes them. Done todos collect in a
  collapsed "Completed" group; a todo ticked not done goes back to the bottom
  of the open todos. Open todos can be reordered by dragging or with the
  keyboard.
- The dashboard stays optimistic: changes appear at once and save in the
  background. A save that fails is undone on screen with a short message.
  Changes made meanwhile in another tab are never silently overwritten:
  - an edit to something changed elsewhere shows the current version and a
    note that it changed elsewhere;
  - an edit to a todo or list deleted elsewhere removes it, with a note that
    it was deleted elsewhere;
  - concurrent moves never lose or duplicate a todo; the list settles on one
    order and both tabs show it.
- Lists and todos have size limits (lists per Member, todos per list, name and
  title length). Reaching one blocks the action with a message naming the
  limit, and nothing is saved.
- A Member's lists and todos are included in their data export. They stay
  usable while a deletion is pending (cancelling keeps them) and are erased
  when the account is purged.
- Lists and todos are private to their Member. Admins and Super Admins can't
  see them through any page or route.
- Todo titles and list names never appear in logs, traces, metrics, browser
  reports or analytics. List pages are recorded in analytics and browser
  measurements as one page (`/dashboard/lists/[id]`), never per list.
- Every list guarantee gets the strongest check that fits it: proofs for the
  pure ordering, validation and version rules, models for the server writes and
  the client queue, and conformance tests that replay model traces through the
  real code. A guarantee that cannot be checked formally names its test and the
  reason.

Mockup (desktop; on mobile the list picker becomes a drawer; counts are open
todos):

```
  /dashboard/lists/<id>
  +---------------+-------------------------------------+
  | Lists      [+]| Work                       [rename] |
  |   Groceries  4| [ add a todo...                   ] |
  | > Work      2 | [ ] ship OT-012                     |
  |   Home      3 | [ ] write OT-013 proposal           |
  |               | > Completed (1)                     |
  +---------------+-------------------------------------+
```

## Capabilities

### New Capabilities

- `lists`: a Member's lists and the todos in them, including ordering, limits,
  conflicting edits, export and erasure.

### Modified Capabilities

- `dashboard`: lists join the dashboard navigation and pages, keep the
  optimistic, contained-failure behaviour, and get addressable list pages.
- `security`: a Member can reach only their own lists and todos, and staff
  can't reach them at all, written down as a security property with a check;
  every list guarantee is proved, modelled or names its test and the reason no
  proof applies.
- `observability`: todo titles and list names join the content telemetry and
  browser reports must never contain; list pages are measured as one page.
- `analytics`: list pages are recorded as one page, not one per list.

## Out of Scope

- **Sharing and collaboration** (shared lists, live updates, invites). This is
  the next change; OT-013 only avoids decisions that would block it.
- **Moving a todo to another list, reordering lists**, and bulk actions such
  as "clear completed".
- **Notes, due dates, reminders, priorities, tags**, and filtering or search.
- **A trash or undo** for deleted lists and todos.
- **Offline use.** Changes need a connection; a failed save is undone.
- **Auditing todo actions.** The audit log records security activity, not
  content changes.
- **Formal proofs of rendering, layout, CSS, SQL text, HTTP wiring or
  third-party code.** Those guarantees name their tests and why no proof or
  model applies.

## Dependencies

- None blocking. OT-012 (backend split into crates) is done and was archived on
  2026-11-23, so implementation can start once planning is complete.
- One additive, forward-only migration. No external services.
- Kani, Quint, Apalache and Tamarin are already pinned for OT-010; the new
  slices extend their models and proofs, and add no services.
- No production deploy exists yet. Deploy checks are recorded for the first
  deploy.

## Appetite

5 weeks.

## Ready

- **Riskiest unknown:** Making optimistic edits safe, and proving it. Rapid
  edits to the same todo in one tab must not conflict with each other, while a
  real conflict from another tab is caught, and a failed save is rolled back
  cleanly. The client queue must be modelled and replayed against the real
  engine without freezing a bug into the model. This is also what collaboration
  will build on.
- **First slice:** A Member creates a list, opens it at its own address, adds
  todos and ticks them done, with conflict-safe saves end to end and the
  lists included in export and erasure.
