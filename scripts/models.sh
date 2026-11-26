#!/usr/bin/env bash
# Quint concurrency models (OT-010 design D8, OT-013 design D13), run by
# `just models` and CI:
#
#   1. Apalache (`quint verify`) checks every invariant over every execution of
#      up to 8 steps, for the code as it stands. The lists server model
#      (list_writes.qnt) is checked to 3 steps, every single request from every
#      initial state: Apalache takes about 4 minutes there and four times as
#      long per further step, and a race needs 6. Random simulation (`quint
#      run`, 100,000 traces of 30 steps) is its gate, and each of its broken
#      variants must fail there.
#   2. The old code's variants must still have counterexamples, so the
#      invariants can't silently become vacuous.
#   3. A deeper random search (`quint run`, 30 steps).
#   4. Random traces, plus witness traces that reach the rare races, are
#      exported as ITF and replayed against Postgres by
#      backend/crates/persistence-postgres/tests/model_conformance.rs, and the
#      list traces also against the web client's in-memory server.
#
# Steps 1-4 are independent, so they run concurrently (up to MODELS_JOBS at a
# time, default: CPU count); only the replay waits for them. Each Apalache check
# starts its own server on its own port, from MODELS_PORT_BASE (default 8840)
# up, since `quint verify` otherwise shares one on 8822. Each job's output is
# printed in a fixed order once all have finished; a failed job prints in full.
#
# Needs quint and java on PATH (pinned in .mise.toml), cargo-nextest, pnpm with
# apps/web's dependencies installed, and a migrated database at DATABASE_URL.
# Run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${DATABASE_URL:?DATABASE_URL must point at a migrated database}"
command -v quint >/dev/null && command -v java >/dev/null ||
  { echo "models: Quint or Java isn't installed: run 'mise install'" >&2; exit 1; }

ac=models/attempt_claim.qnt
rr=models/refresh_rotation.qnt
lw=models/list_writes.qnt
# Every invariant of list_writes.qnt, checked together (`safety` is their
# conjunction; naming them here reports which one broke).
lw_invariants=(staleChangesNothing versionsMoveByOne movesKeepEveryTodoOnce repeatStoresOneRow
  listLimitHolds todoLimitHolds noListForPurgedAccount purgeLeavesNoLists
  writesStayWithTheirMember findListStaysWithItsMember noTodoOutlivesItsList
  listDeleteRefusesChangedCount openPositionsAreUnique)
traces=backend/target/model-traces
max_jobs=${MODELS_JOBS:-$(nproc)}
port=${MODELS_PORT_BASE:-8840}
logs=$(mktemp -d)
cleanup() { # stop any check still running (on failure or Ctrl-C), drop the logs
  local pids
  pids=$(jobs -p)
  [[ -z $pids ]] || kill $pids 2>/dev/null || true
  rm -rf "$logs"
}
trap cleanup EXIT

job=0
spawn() { # description command...: runs the command in the background
  while (($(jobs -rp | wc -l) >= max_jobs)); do wait -n || true; done
  local id
  id=$(printf '%02d' "$job")
  job=$((job + 1))
  echo "$1" >"$logs/$id.what"
  shift
  ( "$@" >"$logs/$id.log" 2>&1 && rc=0 || rc=$?; echo "$rc" >"$logs/$id.rc" ) &
}

collect() { # waits for every job, prints them in order, fails if any failed
  wait
  local failed=0 what id rc
  for what in "$logs"/*.what; do
    id=${what%.what}
    rc=$(cat "$id.rc" 2>/dev/null || echo "?")
    if [[ $rc == 0 ]]; then
      echo "models: $(<"$what"): $(grep -E '^\[(ok|violation)\]' "$id.log" | tail -1)"
    else
      cat "$id.log" >&2
      echo "models: FAILED (exit $rc): $(<"$what")" >&2
      failed=1
    fi
  done
  ((failed == 0))
}

verify() { # file module invariant, on this job's own Apalache port
  quint verify "$1" --main="$2" --invariant "$3" --max-steps "${steps:-8}" --verbosity 1 \
    --server-endpoint "localhost:$port"
}
holds() { verify "$@"; }

simulate() { # file module invariants...: 100,000 random traces of 30 steps
  local file=$1 module=$2
  shift 2
  quint run "$file" --main="$module" --invariants "$@" --max-steps 30 \
    --max-samples "${samples:-100000}" --backend=rust --verbosity 1
}

fails() { # [--run] file module invariant: the variant must break it (Apalache,
  # or with --run random simulation, up to 200,000 traces)
  local check=verify out status
  [[ $1 == --run ]] && { check=simulate; shift; }
  out=$(samples=200000 "$check" "$@" 2>&1) && status=0 || status=$?
  echo "$out"
  if ((status == 0)) || ! grep -q '^\[violation\]' <<<"$out"; then
    echo "models: expected a counterexample to $3 in $2"
    return 1
  fi
}

witnesses() { # module: traces that reach a refused issue
  # Random traces rarely reach a refused issue (the races task 4.1 fixed), so
  # add some that do: each is a counterexample to "no issue is refused". Any
  # other failure (a Quint error, no trace written) fails the job.
  local m=$1 n witness out status
  for n in 1 2 3; do
    witness="$traces/${m}_witness${n}_0.itf.json"
    out=$(quint run $ac --main="$m" --invariant noIssueIsRefused --max-steps 20 \
      --out-itf "$traces/${m}_witness${n}_{seq}.itf.json" --verbosity 1 2>&1) && status=0 || status=$?
    echo "$out"
    if ((status == 0)) || ! grep -q '^\[violation\]' <<<"$out" || [[ ! -s $witness ]]; then
      echo "models: no witness trace in which $m refuses an issue"
      return 1
    fi
  done
}

list_witnesses() { # traces ending in each rare outcome, for the replay and the bound
  local w out status
  for w in noStale noRepeat noListNotFound noTodoNotFound noPurge noSweep \
    noListLimit noTodoLimit noRenumbering noOverlappingMoves; do
    out=$(quint run $lw --main=writes --invariant "$w" --max-steps 30 --max-samples 100000 \
      --backend=rust --out-itf "$traces/list_writes_witness_${w}_{seq}.itf.json" --verbosity 1 2>&1) &&
      status=0 || status=$?
    echo "$out"
    if ((status == 0)) || ! grep -q '^\[violation\]' <<<"$out" ||
      [[ ! -s $traces/list_writes_witness_${w}_0.itf.json ]]; then
      echo "models: list_writes never reaches what $w rules out within 30 steps"
      return 1
    fi
  done
}

rm -rf "$traces" && mkdir -p "$traces"
echo "models: running every check, up to $max_jobs at a time"

# Longest first: the refresh_rotation check takes about as long as the others
# together, and list_writes at 3 steps about as long again.
# Each job's subshell keeps the port it was spawned with.
next() { port=$((port + 1)); spawn "$@"; }
steps=3 next "list_writes keeps every invariant (quint verify, 3 steps)" \
  holds $lw writes safety
next "refresh_rotation keeps safety (quint verify, 8 steps)" holds $rr refresh_rotation safety
next "email_codes keeps safety (quint verify, 8 steps)" holds $ac email_codes safety
next "reset_codes keeps safety (quint verify, 8 steps)" holds $ac reset_codes safety
next "unguarded_upsert breaks lockIsNeverLiftedInsideItsWindow, as it must" \
  fails $ac unguarded_upsert lockIsNeverLiftedInsideItsWindow
next "claim_by_user breaks atMostFiveWrongGuessesPerCode, as it must" \
  fails $ac claim_by_user atMostFiveWrongGuessesPerCode
next "lock_only_guard breaks noReissueInsideTheCooldown, as it must" \
  fails $ac lock_only_guard noReissueInsideTheCooldown

# `quint run` fetches its evaluator from GitHub on first use (CI installs it
# beforehand, since the fetch is unauthenticated and rate-limited). On a fresh
# machine one runs alone first, alongside the Apalache checks above, so it's
# fetched once rather than by every run at once into the same directory.
spawn "refresh_rotation keeps safety (quint run, 30 steps)" \
  quint run $rr --invariant safety --max-steps 30 --max-samples 10000 --verbosity 1
wait "$!" || true

for m in email_codes reset_codes; do
  spawn "$m keeps safety (quint run, 30 steps)" \
    quint run $ac --main=$m --invariant safety --max-steps 30 --max-samples 10000 --verbosity 1
  spawn "$m traces exported" \
    quint run $ac --main=$m --max-steps 20 --n-traces 300 --max-samples 300 \
    --out-itf "$traces/${m}_{seq}.itf.json" --verbosity 1
  spawn "$m witness traces exported" witnesses $m
done
spawn "list_writes keeps every invariant (quint run, 30 steps)" \
  simulate $lw writes "${lw_invariants[@]}"
spawn "list_writes traces exported" \
  quint run $lw --main=writes --max-steps 30 --n-traces 300 --max-samples 300 --backend=rust \
  --out-itf "$traces/list_writes_random_{seq}.itf.json" --verbosity 1
spawn "list_writes witness traces exported" list_witnesses
# Each broken variant of list_writes drops one guard; each must break an invariant.
spawn "no_version_check breaks staleChangesNothing, as it must (quint run)" fails --run $lw no_version_check staleChangesNothing
spawn "no_count_check breaks listDeleteRefusesChangedCount, as it must (quint run)" fails --run $lw no_count_check listDeleteRefusesChangedCount
spawn "no_lock breaks openPositionsAreUnique, as it must (quint run)" fails --run $lw no_lock openPositionsAreUnique
spawn "no_user_lock breaks noListForPurgedAccount, as it must (quint run)" fails --run $lw no_user_lock noListForPurgedAccount
spawn "no_limit breaks todoLimitHolds, as it must (quint run)" fails --run $lw no_limit todoLimitHolds
spawn "no_owner_check breaks writesStayWithTheirMember, as it must (quint run)" fails --run $lw no_owner_check writesStayWithTheirMember
spawn "no_key_check breaks repeatStoresOneRow, as it must (quint run)" fails --run $lw no_key_check repeatStoresOneRow
spawn "no_deleted_check breaks noListForPurgedAccount, as it must (quint run)" fails --run $lw no_deleted_check noListForPurgedAccount
spawn "no_todo_list_check breaks writesStayWithTheirMember, as it must (quint run)" fails --run $lw no_todo_list_check writesStayWithTheirMember
spawn "no_neighbour_scope breaks writesStayWithTheirMember, as it must (quint run)" fails --run $lw no_neighbour_scope writesStayWithTheirMember
spawn "refresh_rotation traces exported" \
  quint run $rr --max-steps 20 --n-traces 300 --max-samples 300 \
  --out-itf "$traces/refresh_rotation_{seq}.itf.json" --verbosity 1

collect

echo "models: replaying the traces against Postgres"
(cd backend && MODEL_TRACES_DIR="$PWD/target/model-traces" \
  cargo nextest run -p persistence-postgres --test model_conformance --run-ignored only)

# The client tests trust MemoryListsServer, so list_writes' traces run through
# it too, for the operations it implements (apps/web/src/lib/lists/server-model.replay.ts).
echo "models: replaying the list traces against MemoryListsServer"
(cd apps/web && MODEL_TRACES_DIR="$OLDPWD/$traces" pnpm exec vitest run --config vitest.models.config.ts)
