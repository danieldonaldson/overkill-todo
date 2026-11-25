# OT-015: Changes save only to the account on screen

## Why

As a Member sharing a browser, I want a change I make to be saved only to the
account I can see, so that my text and settings never end up in someone else's.

All tabs share one session. If another Member signs in from a second tab, the
first tab still shows the old account, and what is done there is saved to the
new one. OT-013 makes this matter: a list name or todo is private text. Its
client checks the account before a list save, but can't catch a switch while a
save is on its way, and nothing protects the other panels (changing a password,
unlinking Google, deleting the account).

## What Changes

- A change made in a tab showing one account is refused, with nothing saved,
  when the session belongs to another.
- The tab then shows the account that is signed in. Lists, unsaved text and
  drafts of the previous account are removed from it, and a message says that
  nothing was saved because another account is signed in.
- This covers every change a signed-in Member, Admin or Super Admin can make.
- A change sent without saying which account it is for is accepted as today, so
  a page opened before the release keeps working.

No new screens. The message uses the existing toast:
`You're signed in as another account now. Nothing was saved.`

## Out of Scope

- Reading. A stale tab that loads data gets the signed-in account's own data.
- Several accounts signed in at once in one browser.
- Keeping the previous account's unsaved text for when they sign in again: the
  next person at that browser could read it, so it is discarded.
- Signing in, signing out, refreshing and browser reports: they don't act on
  the account on screen.

## Appetite

2 days.

## Dependencies

None. Best done before OT-013 slice 2, which adds more list changes.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `security`: changes meant for another account are refused
- `dashboard`: what a tab shows after such a refusal

## Impact

Every authenticated change in the API, the web client that sends them, the API
description and its fuzzing, and the threat model.

## Ready

- **Riskiest unknown:** whether a change keeps the account it was made under
  through the client's own session refresh and resend, when the account changes
  in between.
- **First slice:** the whole check for one real switch, end to end: two tabs, the
  second signs in as another Member while the first has a save on its way.
