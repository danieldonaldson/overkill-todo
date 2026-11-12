# performance Specification

## Purpose
Holds the service to explicit latency and error budgets under load, and lets the
developer measure them repeatably on their own machine so regressions are
found before Members feel them.

## Requirements

### Requirement: Named load scenarios run with one command
The developer SHALL be able to run each load scenario by name with a single
command against the prod-like local stack. The scenarios SHALL be: smoke and
auth mix. Each scenario SHALL
send requests at a set arrival rate, not with a fixed number of users waiting
on each other, so a slow server shows up as higher latency rather than as fewer
requests.

#### Scenario: Run a scenario by name
- **WHEN** the developer runs the load command with the name `smoke` while the prod-like stack is up
- **THEN** load is sent to the stack through the edge for the scenario's duration
- **AND** the command ends with a summary of request count, error rate and p50/p95/p99 latency per route

#### Scenario: Unknown scenario name
- **WHEN** the developer runs the load command with a name that is not a scenario
- **THEN** the command fails at once, lists the valid scenario names and sends no load

#### Scenario: Stack not running
- **WHEN** the developer runs any scenario while the prod-like stack is down or not ready
- **THEN** the command fails within 10 seconds with a message saying the stack is not reachable, and sends no load

#### Scenario: Generator cannot keep up
- **WHEN** the load generator cannot start requests at the scenario's set rate (for example because the machine is saturated)
- **THEN** the summary reports how many planned requests were not sent
- **AND** the run fails rather than reporting results for a lower rate than planned

### Requirement: Load only targets the local stack
A load run SHALL target the local prod-like stack by default. A run SHALL refuse
to send load to any host other than the local machine unless the developer
explicitly opts in for that run.

#### Scenario: Remote target refused
- **WHEN** the developer points a run at a non-local address without the explicit opt-in
- **THEN** the command fails before sending any request and says why

### Requirement: Runs pass or fail against budgets
Each scenario SHALL define limits for p95 latency, p99 latency and error rate on
the routes it exercises. A run SHALL exit with failure when any limit is
exceeded, and SHALL name the limits that were exceeded. Requests answered with an
expected status SHALL NOT count as errors; for example, a check for a deliberately
invalid session that returns 401 is not an error.

#### Scenario: Within budget
- **WHEN** every route's p95, p99 and error rate are within the scenario's limits
- **THEN** the run exits with success

#### Scenario: Latency budget exceeded
- **WHEN** sign-in p95 latency exceeds its limit during the auth mix scenario
- **THEN** the run exits with failure and the summary names sign-in p95 as exceeded, with the measured and allowed values

#### Scenario: Error budget exceeded
- **WHEN** more than the allowed share of requests fail (5xx, timeout, connection error or an unexpected status)
- **THEN** the run exits with failure and the summary names the error-rate limit as exceeded

### Requirement: Scenarios behave like the web client
Load scenarios SHALL authenticate and send requests the way the web client does:
session and refresh cookies, the CSRF token returned at sign-in sent back on
state-changing requests, and the device fingerprint. Each simulated Member SHALL
have its own session. A scenario SHALL NOT reach the backend except through the
edge.

#### Scenario: Authenticated state-changing request succeeds
- **WHEN** a simulated Member signs in and then signs out
- **THEN** both requests succeed with the same status the web client would get

#### Scenario: Session refresh rotates the token
- **WHEN** a simulated Member refreshes their session twice in a row
- **THEN** each refresh succeeds and uses the refresh token issued by the previous one

### Requirement: Test Members are created through the real sign-up flow
Before a scenario that needs existing accounts, the developer SHALL be able to
create a set number of verified test Members through the same sign-up and
verification steps a Visitor uses. Seeding SHALL be safe to repeat: accounts that
already exist are reused, not duplicated or failed on. Test Members SHALL use
addresses that cannot belong to real people.

#### Scenario: Seed from empty
- **WHEN** the developer seeds 500 test Members on an empty database
- **THEN** 500 verified accounts exist and each can sign in with the known test password

#### Scenario: Seed again
- **WHEN** the developer seeds 500 test Members when 300 already exist
- **THEN** the 300 are reused, 200 are created, and the command succeeds

#### Scenario: Scenario run without seeding
- **WHEN** the developer runs a scenario that needs existing Members before seeding
- **THEN** the run fails at once with a message telling the developer to seed first, and sends no load

### Requirement: Load-tested stack is resource-capped and prod-equivalent
During a load run, the backend and database SHALL run with fixed CPU and memory
limits, and password hashing SHALL use production-equivalent cost. Every run
SHALL start from the same state: sessions left over from earlier runs by test
Members SHALL be cleared, and a measured run SHALL NOT start until the stack has
had a set idle period since the previous load run, which the developer can
shorten or skip. The summary SHALL record the
limits and hashing cost used, so runs are only compared like with like.

#### Scenario: Sign-in cost is realistic
- **WHEN** the auth mix scenario runs
- **THEN** password checks use the same hashing cost as production, not the cheaper local-development cost

#### Scenario: Earlier runs don't slow later ones
- **WHEN** the developer runs auth mix several times in a row
- **THEN** each run starts with no sessions left over from the previous runs, and refresh and sign-out latency don't climb from run to run because of them

#### Scenario: Back-to-back runs wait for an idle stack
- **WHEN** the developer starts a scenario less than the idle period after the previous load run ended
- **THEN** the command says how long it will wait, waits, and only then sends load

#### Scenario: Limits recorded with results
- **WHEN** any run finishes
- **THEN** its saved summary includes the backend and database CPU and memory limits, the hashing cost, the scenario name, the date and the commit

### Requirement: Results are kept for comparison
Each finished run SHALL save a machine-readable summary. When the
observability stack is running, a run SHALL also send the load generator's
metrics to it. A run SHALL work, pass and fail the same way whether or not the
observability stack is running.

#### Scenario: Observability stack down
- **WHEN** a scenario runs without the observability stack
- **THEN** the run completes, prints its summary, saves it, and passes or fails as usual

#### Scenario: Compare with baseline
- **WHEN** a run finishes and a baseline summary exists for that scenario
- **THEN** the printed summary shows each route's p95 next to the baseline's p95 and the change in percent

#### Scenario: Set a baseline
- **WHEN** the developer marks the latest complete run of a scenario as its baseline
- **THEN** later runs of that scenario compare against it, and the previous baseline is replaced

#### Scenario: No baseline yet
- **WHEN** a run finishes and no baseline exists for that scenario
- **THEN** the summary says there is no baseline to compare against, and the run passes or fails on its limits alone

### Requirement: Interrupted runs leave nothing behind
Stopping a run partway through SHALL end the load at once, print a summary of
what was measured so far marked as incomplete, and leave no load-generator
process or container running. An incomplete run SHALL NOT be saved as a baseline.

#### Scenario: Developer interrupts a run
- **WHEN** the developer interrupts the auth mix scenario partway through
- **THEN** load stops within 5 seconds, a summary marked incomplete is printed, and no load-generator container is left running
