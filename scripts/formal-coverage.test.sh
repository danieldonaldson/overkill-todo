#!/usr/bin/env bash
# Fixture tests for scripts/formal-coverage.sh (OT-013 task 6.1), run by
# `just formal-coverage` before the gate itself. Each test builds a small
# repository that passes, breaks one thing, and expects the gate to fail with a
# message naming it (or, for the passing cases, to succeed).
set -euo pipefail
script=$(realpath "$(dirname "$0")/formal-coverage.sh")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0

# A repository the gate accepts: one active change with one capability, every
# kind of check, and one scenario pending on an unticked task.
fixture() {
  local root=$work/$1
  rm -rf "$root"
  mkdir -p "$root"/{openspec/changes/demo/specs/lists,docs/security,scripts,models,src,web}
  cd "$root"
  cat >openspec/changes/demo/specs/lists/spec.md <<'EOF'
# Spec Delta

## ADDED Requirements

### Requirement: Order is kept
Text.

#### Scenario: Move
- **WHEN** x
- **THEN** y

#### Scenario: Concurrent moves
- **WHEN** x
- **THEN** y

### Requirement: Titles render
Text.

#### Scenario: Markup
- **WHEN** x
- **THEN** y

## REMOVED Requirements

### Requirement: Old thing
#### Scenario: Gone
EOF
  cat >openspec/changes/demo/tasks.md <<'EOF'
## 1. Slice
- [x] 1.1 Done task
- [ ] 1.2 Open task
EOF
  printf 'test:\n    true\nformal-coverage:\n    true\n' >justfile
  cat >docs/security/threat-model.md <<'EOF'
### P-35: Lists are private
EOF
  cat >scripts/models.sh <<'EOF'
q=models/order.qnt
holds $q order orderIsTotal
# holds $q order commentedOut
fails $q no_lock orderIsTotal
cargo nextest run --test model_conformance --run-ignored only
EOF
  cat >models/order.qnt <<'EOF'
module order {
  val orderIsTotal = true
  val commentedOut = true
  val unchecked = true
}
module no_lock {
  import order.*
}
EOF
  cat >src/order.rs <<'EOF'
/// Spec: lands between. Bound: every i64; no assumptions.
#[kani::proof]
#[kani::unwind(3)]
fn a_move_lands_between() {}

/// Spec: no harness attribute. Bound: every i64.
fn not_a_harness() {}

#[kani::proof]
fn harness_without_bounds() {}

#[cfg(test)]
mod tests {
    #[test]
    fn moves_keep_order() {}

    #[sqlx::test(
        migrations = "x"
    )]
    async fn concurrent_moves_keep_order() {}

    #[test]
    #[ignore = "slow"]
    fn ignored_test() {}
}
EOF
  cat >src/model_conformance.rs <<'EOF'
#[test]
#[ignore = "run by just models"]
fn replays_order_traces() {}
EOF
  cp src/model_conformance.rs src/other_replay.rs
  cat >web/titles.test.ts <<'EOF'
describe('titles', () => {
  it('renders markup as text', () => {})
  it.skip('a skipped one', () => {})
  test(
    'a long title on its own line',
    () => {},
  )
})
EOF
  cat >scripts/self.test.sh <<'EOF'
test_one() { true; }
test_one
EOF
  cat >docs/formal-coverage.yaml <<'EOF'
change: demo
capabilities:
  lists:
    Order is kept:
      scenarios:
        Move:
          checks:
            - kani: src/order.rs::a_move_lands_between
            - rust: src/order.rs::moves_keep_order
        Concurrent moves:
          checks:
            - invariant: models/order.qnt::orderIsTotal
            - broken: models/order.qnt::no_lock
            - replay: src/model_conformance.rs::replays_order_traces
            - rust: src/order.rs::concurrent_moves_keep_order
    Titles render:
      reason: Rendering is browser behaviour; no proof or model applies.
      scenarios:
        Markup:
          checks:
            - web: web/titles.test.ts::renders markup as text
            - web: web/titles.test.ts::a long title on its own line
            - just: test
            - shell: scripts/self.test.sh::test_one
properties:
  P-35:
    checks:
      - rust: src/order.rs::moves_keep_order
EOF
}

# expect <name> pass|fail [message substring] [gate args...]: runs the gate in
# the current fixture.
expect() {
  local name=$1 want=$2 needle=${3:-} out status
  shift 3 || shift $#
  out=$(FORMAL_COVERAGE_ROOT=$PWD "$script" "$@" 2>&1) && status=0 || status=$?
  if [[ $want == pass && $status != 0 ]]; then
    echo "FAIL $name: expected the gate to pass, got:"$'\n'"$out"
  elif [[ $want == fail && $status == 0 ]]; then
    echo "FAIL $name: expected the gate to fail, got:"$'\n'"$out"
  elif [[ -n $needle && $out != *"$needle"* ]]; then
    echo "FAIL $name: expected '$needle' in:"$'\n'"$out"
  else
    echo "ok   $name"
    return 0
  fi
  failures=$((failures + 1))
}
yset() { yq -i "$1" docs/formal-coverage.yaml; }
L='.capabilities.lists'

test_a_complete_manifest_passes() {
  fixture complete
  expect "a complete manifest passes" pass "ok (delta specs, 3 scenarios, 0 pending, 0 exceptions)"
}

test_an_empty_manifest_fails() {
  fixture empty
  printf 'change: demo\n' >docs/formal-coverage.yaml
  expect "an empty manifest fails" fail "lists / Order is kept: requirement has no entry"
}

test_a_missing_scenario_fails() {
  fixture missing-scenario
  yset "del(${L}[\"Order is kept\"].scenarios.Move)"
  expect "a missing scenario fails" fail "lists / Order is kept / Move: scenario has no entry"
}

test_a_renamed_scenario_fails() {
  fixture renamed-scenario
  sed -i 's/^#### Scenario: Move$/#### Scenario: Drag/' openspec/changes/demo/specs/lists/spec.md
  expect "a renamed scenario fails" fail "Order is kept / Move: no such scenario in the specs"
}

test_a_removed_requirement_is_not_demanded() {
  fixture removed
  expect "a REMOVED requirement isn't demanded" pass ""
  yset "${L}[\"Old thing\"].scenarios.Gone.checks = [{\"just\": \"test\"}] | ${L}[\"Old thing\"].reason = \"x\""
  expect "an entry for a REMOVED requirement fails" fail "Old thing: no such requirement"
}

test_a_renamed_check_fails() {
  fixture renamed-check
  sed -i 's/fn moves_keep_order/fn moves_keep_their_order/' src/order.rs
  expect "a renamed check fails" fail "no fn moves_keep_order in src/order.rs"
  fixture renamed-web
  sed -i "s/renders markup as text/renders markup safely/" web/titles.test.ts
  expect "a renamed web test fails" fail "no test 'renders markup as text'"
}

test_a_missing_file_fails() {
  fixture missing-file
  rm web/titles.test.ts
  expect "a missing file fails" fail "no file web/titles.test.ts"
}

test_an_ignored_or_skipped_test_fails() {
  fixture ignored
  yset "${L}[\"Order is kept\"].scenarios.Move.checks[1].rust = \"src/order.rs::ignored_test\""
  expect "an ignored Rust test fails" fail "src/order.rs::ignored_test is ignored"
  fixture skipped
  yset "${L}[\"Titles render\"].scenarios.Markup.checks[0].web = \"web/titles.test.ts::a skipped one\""
  expect "a skipped web test fails" fail "'a skipped one' is skip"
  fixture skipped-describe
  sed -i "s/describe('titles'/describe.skip('titles'/" web/titles.test.ts
  expect "a skipped describe fails" fail "has a skipped describe"
  fixture cfg-ignored
  perl -0pi -e 's/    #\[test\]\n    fn moves_keep_order/    #[test]\n    #[cfg_attr(\n        not(feature = "slow"),\n        ignore\n    )]\n    fn moves_keep_order/' src/order.rs
  expect "a test ignored through cfg_attr fails" fail "src/order.rs::moves_keep_order is ignored"
  fixture conditional-skip
  perl -0pi -e "s/it\('renders markup as text', \(\) => \{\}\)/it('renders markup as text', () => {\n    test.skip(process.env.CI !== undefined, 'flaky')\n  })/" web/titles.test.ts
  expect "a conditional skip in a test body fails" fail "has a conditional skip"
  fixture ctx-skip
  perl -0pi -e "s/it\('renders markup as text', \(\) => \{\}\)/it('renders markup as text', (ctx) => {\n    ctx.skip()\n  })/" web/titles.test.ts
  expect "ctx.skip() fails" fail "has a conditional skip"
  fixture integration
  mkdir -p web/tests/integration
  cp web/titles.test.ts web/tests/integration/titles.spec.ts
  yset "${L}[\"Titles render\"].scenarios.Markup.checks[0].web = \"web/tests/integration/titles.spec.ts::renders markup as text\""
  expect "a flow just ci doesn't run fails" fail "isn't run by \`just ci\`"
  fixture not-a-test
  yset "${L}[\"Order is kept\"].scenarios.Move.checks[1].rust = \"src/order.rs::not_a_harness\""
  expect "a plain fn isn't a test" fail "not_a_harness isn't a test"
}

test_a_replay_must_be_run_by_models() {
  fixture replay
  yset "${L}[\"Order is kept\"].scenarios[\"Concurrent moves\"].checks[2].replay = \"src/other_replay.rs::replays_order_traces\""
  expect "an ignored replay models.sh doesn't run fails" fail "scripts/models.sh doesn't run other_replay"
}

test_a_harness_without_kani_proof_fails() {
  fixture no-proof
  yset "${L}[\"Order is kept\"].scenarios.Move.checks[0].kani = \"src/order.rs::not_a_harness\""
  expect "a harness without #[kani::proof] fails" fail "not_a_harness has no #[kani::proof]"
  fixture no-bounds
  yset "${L}[\"Order is kept\"].scenarios.Move.checks[0].kani = \"src/order.rs::harness_without_bounds\""
  expect "a harness without stated bounds fails" fail "doesn't state its bounds or assumptions"
}

test_an_invariant_models_sh_does_not_run_fails() {
  fixture unchecked
  yset "${L}[\"Order is kept\"].scenarios[\"Concurrent moves\"].checks[0].invariant = \"models/order.qnt::unchecked\""
  expect "an invariant models.sh never checks fails" fail "scripts/models.sh doesn't check unchecked"
  fixture commented
  yset "${L}[\"Order is kept\"].scenarios[\"Concurrent moves\"].checks[0].invariant = \"models/order.qnt::commentedOut\""
  expect "an invariant only in a comment fails" fail "scripts/models.sh doesn't check commentedOut"
  fixture broken
  sed -i 's/^fails \$q no_lock/holds $q no_lock/' scripts/models.sh
  expect "a broken variant models.sh doesn't expect to fail fails" fail "doesn't expect no_lock to fail"
}

test_pending_entries() {
  fixture pending-open
  yset "${L}[\"Order is kept\"].scenarios.Move = {\"pending\": \"1.2\"}"
  expect "pending on an unticked task passes" pass "1 pending"
  expect "--final refuses any pending entry" fail "still pending (1.2), and --final allows none" --final
  fixture pending-ticked
  yset "${L}[\"Order is kept\"].scenarios.Move = {\"pending\": \"1.1\"}"
  expect "pending on a ticked task fails" fail "pending 1.1, but 1.1 is ticked"
  fixture pending-unquoted
  yset "${L}[\"Order is kept\"].scenarios.Move = {\"pending\": 1.2}"
  expect "an unquoted task number fails" fail "pending must be a quoted task number"
  fixture pending-missing
  yset "${L}[\"Order is kept\"].scenarios.Move = {\"pending\": \"9.9\"}"
  expect "pending on a task that doesn't exist fails" fail "has no task 9.9"
}

test_exceptions() {
  fixture exception
  yset "${L}[\"Titles render\"].scenarios.Markup = {\"exception\": \"Measured by hand: needs the prod-like stack.\"}"
  expect "an exception passes" pass "0 pending, 1 exceptions"
  expect "an exception passes --final" pass "1 exceptions" --final
  fixture exception-empty
  yset "${L}[\"Titles render\"].scenarios.Markup = {\"exception\": \"  \"}"
  expect "an empty exception fails" fail "exception must say why no automated check exists"
  fixture exception-with-checks
  yset "${L}[\"Titles render\"].scenarios.Markup.exception = \"x\""
  expect "an exception beside checks fails" fail "an exception can't sit beside checks or pending"
  fixture exception-with-pending
  yset "${L}[\"Titles render\"].scenarios.Markup = {\"pending\": \"1.2\", \"exception\": \"x\"}"
  expect "an exception beside pending fails" fail "an exception can't sit beside checks or pending"
}

test_a_requirement_without_formal_check_or_reason_fails() {
  fixture no-reason
  yset "del(${L}[\"Titles render\"].reason)"
  expect "a test-only requirement without a reason fails" fail "Titles render: no proof, model or replay, and no reason"
}

test_the_manifest_shape_is_strict() {
  fixture typo
  yset "${L}[\"Order is kept\"].scenarios.Move.chekcs = []"
  expect "an unknown key fails" fail "unknown key 'chekcs'"
  fixture kind
  yset "${L}[\"Order is kept\"].scenarios.Move.checks[0] = {\"proof\": \"src/order.rs::a_move_lands_between\"}"
  expect "an unknown check kind fails" fail "unknown check kind 'proof'"
  fixture both
  yset "${L}[\"Order is kept\"].scenarios.Move.pending = \"1.2\""
  expect "checks and pending together pass" pass "1 pending"
  sed -i 's/fn moves_keep_order/fn moves_keep_their_order/' src/order.rs
  expect "checks beside pending are still run" fail "no fn moves_keep_order"
  fixture neither
  yset "${L}[\"Order is kept\"].scenarios.Move = {}"
  expect "a scenario with neither checks nor pending fails" fail "give checks, pending, or both (or an exception)"
  fixture scalar-check
  yset "${L}[\"Order is kept\"].scenarios.Move.checks[0] = \"src/order.rs::a_move_lands_between\""
  expect "a check without a kind fails" fail "check 1 must be one 'kind: target'"
  fixture two-keys
  yset "${L}[\"Order is kept\"].scenarios.Move.checks[0].rust = \"src/order.rs::moves_keep_order\""
  expect "a check with two kinds fails" fail "check 1 must be one 'kind: target'"
  fixture empty-checks
  yset "${L}[\"Order is kept\"].scenarios.Move.checks = []"
  expect "an empty list of checks fails" fail "Order is kept / Move: no checks"
  fixture checks-scalar
  yset "${L}[\"Order is kept\"].scenarios.Move.checks = \"src/order.rs::moves_keep_order\""
  expect "checks that aren't a list fail" fail "checks must be a list"
  fixture top-level
  yset ".capabilites = {}"
  expect "an unknown top-level key fails" fail "unknown top-level key 'capabilites'"
}

test_the_threat_model_property_must_exist() {
  fixture property
  printf '### P-36: Something else\n' >docs/security/threat-model.md
  expect "a property missing from the threat model fails" fail "P-35: not in docs/security/threat-model.md"
}

test_the_archived_layout() {
  fixture archived
  mkdir -p openspec/specs/lists openspec/changes/archive
  # The merged spec also holds another feature's requirement, not demanded.
  sed -e 's/^## ADDED Requirements/## Requirements/' -e '/^## REMOVED/,$d' \
    openspec/changes/demo/specs/lists/spec.md >openspec/specs/lists/spec.md
  printf '\n### Requirement: Another feature\n\n#### Scenario: Elsewhere\n' >>openspec/specs/lists/spec.md
  sed -i 's/- \[ \]/- [x]/' openspec/changes/demo/tasks.md
  mv openspec/changes/demo openspec/changes/archive/2026-12-01-demo
  expect "the archived layout reads openspec/specs" pass "ok (archived specs, 3 scenarios, 0 pending, 0 exceptions)"
  sed -i 's/^#### Scenario: Move$/#### Scenario: Drag/' openspec/specs/lists/spec.md
  expect "an archived scenario renamed fails" fail "Order is kept / Move: no such scenario"
  fixture archived-pending
  mkdir -p openspec/specs/lists openspec/changes/archive
  sed -e 's/^## ADDED Requirements/## Requirements/' openspec/changes/demo/specs/lists/spec.md >openspec/specs/lists/spec.md
  mv openspec/changes/demo openspec/changes/archive/2026-12-01-demo
  sed -i 's/- \[ \]/- [x]/' openspec/changes/archive/2026-12-01-demo/tasks.md
  yset "${L}[\"Order is kept\"].scenarios.Move = {\"pending\": \"1.2\"}"
  expect "a pending entry after archiving fails" fail "pending 1.2, but 1.2 is ticked"
}

for test in $(declare -F | awk '{print $3}' | grep '^test_'); do
  "$test"
done
((failures == 0)) || { echo "formal-coverage tests: $failures failed" >&2; exit 1; }
echo "formal-coverage tests: all passed"
