# How features are shaped and built

[OpenSpec](https://github.com/Fission-AI/OpenSpec) keeps the _why_ and _what_
of a feature next to the code, so the reasoning survives after it ships.
`openspec/specs/` describes what the app does today; `openspec/changes/` holds
features in flight.

Not everything needs a change. Dependency bumps, small bugs and refactors are
just commits. Write a change when the work alters behaviour someone relies on,
or will take more than one slice.

## Features, capabilities and slices

- **A feature is one change**: one outcome, one appetite. Its folder is
  `openspec/changes/ot-<nnn>-<name>/` and its proposal starts
  `# OT-<nnn>: <name>`. Take the next number after the highest one in
  `changes/`, including `archive/`; never reuse one.
- **A capability is a lasting product area** (`auth`, `accounts`, `lists`)
  with one spec under `openspec/specs/`. A change carries deltas to the
  capabilities it touches; archiving merges them in.
- **A slice is one numbered group in `tasks.md`**: some of the spec's
  scenarios, end to end, deployable on its own, about 5 working days at most.

```
openspec/changes/ot-004-shared-lists/
  proposal.md            why, appetite, out of scope, Ready section
  specs/lists/spec.md    scenarios (WHEN/THEN), unhappy path included
  design.md              how: hypothesis, decisions, migration plan
  tasks.md
    ## 1. Slice 1 (first slice): owner shares a list with one member
    ## 2. Slice 2: revoke access; shared member can't delete
    ## 3. Slice 3: limits, concurrent edits, mobile
    ## 4. Feature done: switch on, docs, archive
```

**Slice by scenario.** The first slice is the thin happy path that also
retires the riskiest unknown. Later slices add permissions, edge cases and
error handling. If a slice is still too big, group its scenarios differently:

- **By workflow step:** create → view → edit → delete
- **By user type:** Member before Admin
- **By happy/unhappy path:** core flow first, error handling next
- **By data scope:** a single item before bulk operations
- **By platform:** desktop first, mobile next

Also:

- A slice doesn't have to be visible to users, but it must be deployable and
  checkable in production (a live migration, an endpoint not yet linked from
  the UI).
- A small feature can be a single slice.
- **A large feature stays one change.** If the appetite is over about 6
  weeks, the proposal adds a "Shape of the work" section: a suggested order of
  parts, each releasable on its own and likely several slices.

## Before starting

**Feature: shaped**

- [ ] Who it's for and why it matters are explicit
- [ ] Appetite set: how much time it is worth, not an estimate
- [ ] Out of scope listed
- [ ] Specs have testable scenarios, including the unhappy path (below)
- [ ] Dependencies identified: external services, migrations, blocking work
- [ ] UI clear, if user-facing: a mockup or a clear description

**Slice: ready**

- [ ] The scenarios this slice delivers are named
- [ ] For the first slice: the riskiest unknown is named and this slice
      retires it
- [ ] No more than about 5 days of work; if not, split it or narrow the
      scenarios

If you can't name the riskiest unknown and describe the first slice, the
feature isn't understood well enough yet: `/opsx:explore` it first.

### The unhappy path

Most scope surprises come from undefined error states. Every spec should
answer, or say why it can't happen:

- What happens when validation fails?
- What if Postgres or an external service (email, Google, Turnstile) is down?
- Empty state / no data?
- Who may do this (Visitor, Member, Admin, Super Admin), and can one Member
  reach another's data?
- One item vs. 10,000?
- Two tabs or two people changing the same thing at once?
- Leaving part-way through?
- Mobile?

### Spikes

Prefer a first slice to a spike: real work that retires the riskiest unknown
and gets deployed. Use a pure spike (time-boxed, at most 2 days, answering
specific questions) only when nothing deployable can come out of it.

## design.md

Written in two passes:

1. **Before the first slice.** Enough to say which approach and why, and what
   the first slice is testing. `tasks.md` gets the first slice only.
2. **After the first slice ships.** Record what it showed, revise the
   decisions it changed, then add the remaining slices to `tasks.md`. Keep
   updating Decisions as later slices settle things.

Sections:

- **Context:** current state and constraints. Point to the proposal for
  motivation rather than restating it.
- **Goals / Non-Goals:** design-level boundaries only.
- **Hypothesis:** what you believe about the riskiest unknown, how and where
  you'll test it, and how you'll know. Production behind a flag is often the
  right place; test earlier when being wrong would be costly or hard to undo.
  After the first slice, add what actually happened.
- **Decisions:** each choice, why, and the alternatives. This is the part
  that matters most, because it's what you'll have forgotten in six months.
  Name the layer (domain / application / infrastructure) for new pieces.
- **Risks / Trade-offs** as `[Risk] → Mitigation`.
- **Migration Plan:** the expand → deploy → contract steps and rollback for
  any schema change.
- **Open Questions:** only unknowns that can safely wait. If the answer would
  change the specs, the approach or the slices, settle it now.

Keep it proportional: a simple change needs a short Context, the Hypothesis
and one or two Decisions.

## Done

**Slice: deployed**

- [ ] Tests cover the slice's scenarios, including its edge cases
- [ ] `just ci` green; any shortcut taken has a follow-up issue
- [ ] Non-obvious logic and workarounds explained in code
- [ ] README/docs updated for new env vars, setup steps or API changes
- [ ] Migrations compatible with the previous release, with a rollback plan
- [ ] Behind a flag if not yet ready for users
- [ ] Deployed (digest committed to the deploy config) and the scenarios
      checked in production (`just smoke <url>` where it applies)
- [ ] Logging or alerts in place if the slice adds something that can fail

**Feature: done**

- [ ] All slices deployed
- [ ] Checked against the specs in production, not just reasoned about
- [ ] Switched on; flag removed or its removal scheduled
- [ ] Change archived (`/opsx:archive`), so `openspec/specs/` reflects what
      the app now does

### What Done is NOT

| Not done                              | Actually done                                       |
| ------------------------------------- | --------------------------------------------------- |
| "Works on my machine"                 | Verified in production                              |
| "Merged, waiting for the next deploy" | Deployed                                            |
| "Feature is done, tests are TODO"     | Tests written and passing                           |
| "Done except for the edge case"       | All the slice's scenarios work, edge cases included |

### When to flex

The checklists are a standard, not a straitjacket. Bug fixes may not need
docs or flags; spikes produce findings, not code; hotfixes may skip steps,
but the tests and docs follow immediately after. When you skip something,
note it in the commit or the change.

## Defaults

So that decisions don't stall:

| Situation                               | Default                                                |
| --------------------------------------- | ------------------------------------------------------ |
| The appetite runs out                   | Cut scope and ship what works                          |
| A slice is in progress for over ~5 days | Split it                                               |
| A new requirement turns up mid-slice    | It becomes a new slice or issue, not absorbed silently |
| A detail of the spec is unclear         | Decide, and record it in `design.md` Decisions         |

## Commands

| Command         | Use it to                                                 |
| --------------- | --------------------------------------------------------- |
| `/opsx:explore` | think a problem through before committing to a change     |
| `/opsx:propose` | create a change with proposal, specs, design and tasks    |
| `/opsx:update`  | revise artifacts of an existing change                    |
| `/opsx:apply`   | implement the tasks, slice by slice                       |
| `/opsx:sync`    | merge a change's spec deltas into `openspec/specs/` early |
| `/opsx:archive` | finish a change and merge its specs                       |

The rules in `openspec/config.yaml` steer the generated artifacts toward this
workflow. The commands need the `openspec` CLI (installed through mise).
