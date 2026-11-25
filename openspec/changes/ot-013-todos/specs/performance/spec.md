# Spec Delta

## MODIFIED Requirements

### Requirement: Named load scenarios run with one command
The developer SHALL be able to run each load scenario by name with a single
command against the prod-like local stack. The scenarios SHALL be: smoke, auth
mix, lists, and large lists (1,000 todos in one list). Each scenario SHALL
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

#### Scenario: List scenarios start from the same lists
- **WHEN** the developer runs a list scenario twice
- **THEN** both runs start with the same number of lists and todos, whatever earlier runs left behind
