# Tasks

## 1. Slice 1 (first slice): refuse and recover, end to end

Retires the riskiest unknown: the account is kept through refresh and resend.

Delivers these scenarios:

- **security:** all.
- **dashboard:** all.

- [ ] 1.1 Server: compare `X-Account-Id` in `auth_middleware` for changing methods, `AppError` for 412 `account_changed`, document it and the header in `require_auth()`, allow the header in CORS (D1–D3). Verify with integration tests: other account, same, none, malformed, empty, repeated, Visitor, a GET, an unknown account's ID answered identically to a real one, and nothing saved.
- [ ] 1.2 Guard test over the API description: every changing operation that requires the session documents 412 and answers it, as Member, Admin and Super Admin as the route allows (D3). Verify that mounting one without `require_auth()` in the test fails it.
- [ ] 1.3 Run `just api-types` and commit both files. Fuzz hooks send the right ID, leave it out sometimes, and check that another account's gives 412 (D4). Verify with `just fuzz-api-test` and `just fuzz-api`.
- [ ] 1.4 Threat model: add the property with the tests from 1.1 and 1.2. Verify with `just threat-model-check`.
- [ ] 1.5 Client: take the ID once and keep it through resends; `AccountChangedError` (D5). Verify with Vitest: 401 resend, `csrf_invalid` resend, refresh before the first send, no cached account, reads without the header.
- [ ] 1.6 Session store: clear first, check, one toast, Admin pages re-evaluated (D6). Vitest: one message for three queued list changes, `/auth/me` unavailable, each Settings change, an Admin change, and a change right after signing in in the same tab.
- [ ] 1.7 Flows: Playwright with the API mocked for the refusal on a list and on Settings; integration flow from the Hypothesis on the real backend, also confirmed to fail when the ID is read per attempt. Verify with `just e2e` and `just e2e-integration`.
- [ ] 1.8 Telemetry: count `account_changed` by route template, and alert on a sustained rate. Verify by forcing one locally; no ID in metric labels.
- [ ] 1.9 Record the Hypothesis result in design.md. Get `just ci` green. Note for the first deploy: server before web, `just smoke <url>`, then the two-tab flow by hand.

## 2. Feature done

- [ ] 2.1 Remove `ensureSameAccount` and `lists.guardSaves`, keeping the `storage` event (D7), and update OT-013 design D7. Verify with `just test-web` and the integration flow.
- [ ] 2.2 Update README and `backend/ARCHITECTURE.md` for the header, get `just ci` green, and archive the change.
