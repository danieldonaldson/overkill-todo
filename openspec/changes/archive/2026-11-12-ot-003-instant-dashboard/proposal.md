# OT-003: Instant dashboard

## Why

As a Member, I want the dashboard to appear immediately and stay put as I move
between its pages, so that it feels like one fast app rather than a series of
page loads behind a spinner.

Today the dashboard is pessimistic. It shows a full-screen "Loading..." spinner
until the browser has confirmed who is signed in, on the first load and again
on every move between Home, Settings and Admin. Each move also tears down and
rebuilds the header and navigation. Almost every visit is from a Member whose
session is fine, so they all wait for a check that nearly always passes.

The signed-in account is also held in three separate places in the browser,
which already drift: setting a password in Settings does not update what the
header knows.

This change makes the dashboard optimistic: assume the Member is signed in,
show the dashboard at once, and handle the rare failure when it happens. It
keeps the static, page-per-route site and its strict script policy. Turning
the dashboard into a single client-side app stays an option for later.

## What Changes

- A browser with a session shows the dashboard immediately, on the first load
  and on every move. The account check runs in the background. If it fails, the
  Member is sent to sign-in or email verification, as today.
- A browser that has never signed in (or has signed out) goes straight to
  sign-in without seeing the dashboard.
- The site root opens the dashboard rather than the sign-in page, so a Member
  with a session never sees sign-in flash on the way in.
- Parts that show account details (email, Admin entry, Settings options) use
  what the browser remembers from earlier in the tab. With nothing remembered,
  they show a placeholder until the check answers.
- If the background check cannot reach the server, the Member stays on the
  dashboard and sees a notice with a retry option, instead of being signed out.
- The header and navigation stay on screen, unchanged, while a Member moves
  between dashboard pages. Only the page content changes.
- There is one record of the signed-in account in the tab. Changes made on one
  page show everywhere on the dashboard straight away.
- Signing out returns the Member to sign-in without waiting long for the server.

## Capabilities

### New Capabilities
- `dashboard`: the signed-in area of the app: its shared header and navigation,
  how its pages appear and how a Member moves between them, and how it knows
  who is signed in.

### Modified Capabilities
None.

## Appetite

Four days.

## Out of Scope

- Optimistic actions (showing an action's result before the server confirms it
  and undoing it on failure). That becomes its own change, built as a shared
  pattern that the todo feature will rely on.
- Turning the dashboard into a single client-side app, or changing the sign-in
  and other public pages.
- Restructuring dashboard code into feature modules with view-models.
- Changing how sessions, token refresh or CSRF work on the server.
- Loading below-the-fold content only when it scrolls into view. No current
  page has any; the design records the rule for pages that will.

## Dependencies

None. No backend changes, external services or data migrations.

## Ready

- **Riskiest unknown:** whether the dashboard can be shown before the check
  without exposing anything it shouldn't, or flashing wrong content (another
  account's details, an Admin entry for a Member, a dashboard for a Visitor)
  in the cases where the optimistic guess is wrong.
- **First slice:** the whole feature, which fits the appetite. It starts with
  the header staying in place and the first load showing content with no
  spinner. Browser tests then cover each wrong-guess case: no session, an
  expired session, an unverified email, a Member on the Admin page, and the
  server being down.

## Impact

Frontend only (`apps/web`): the dashboard layout and header, the sign-in guard
(removed), the browser-side record of the signed-in account, the Home,
Settings and Admin panels, and the dashboard browser tests.
