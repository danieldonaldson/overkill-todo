#!/usr/bin/env bash
# Quint concurrency models (OT-010 design D8), run by `just models` and CI:
#
#   1. Apalache (`quint verify`) checks every invariant over every execution of
#      up to 8 steps, for the code as it stands.
#   2. The old code's variants must still have counterexamples, so the
#      invariants can't silently become vacuous.
#   3. A deeper random search (`quint run`, 30 steps).
#   4. Random traces, plus witness traces that reach the rare races, are
#      exported as ITF and replayed against Postgres by
#      backend/tests/model_conformance.rs.
#
# Needs quint and java on PATH (pinned in .mise.toml), cargo-nextest, and a
# migrated database at DATABASE_URL. Run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${DATABASE_URL:?DATABASE_URL must point at a migrated database}"
command -v quint >/dev/null && command -v java >/dev/null ||
  { echo "models: Quint or Java isn't installed: run 'mise install'" >&2; exit 1; }

ac=models/attempt_claim.qnt
rr=models/refresh_rotation.qnt
traces=backend/target/model-traces

holds() { # file module invariant
  echo "models: $2 keeps $3 (quint verify, 8 steps)"
  quint verify "$1" --main="$2" --invariant "$3" --max-steps 8 --verbosity 1
}

fails() { # file module invariant: the old code's variant must break it
  local out status
  echo "models: $2 breaks $3, as it must (quint verify, 8 steps)"
  out=$(quint verify "$1" --main="$2" --invariant "$3" --max-steps 8 --verbosity 1 2>&1) && status=0 || status=$?
  if ((status == 0)) || ! grep -q '^\[violation\]' <<<"$out"; then
    echo "$out" >&2
    echo "models: expected a counterexample to $3 in $2" >&2
    exit 1
  fi
}

holds $ac email_codes safety
holds $ac reset_codes safety
holds $rr refresh_rotation safety
fails $ac unguarded_upsert lockIsNeverLiftedInsideItsWindow
fails $ac claim_by_user atMostFiveWrongGuessesPerCode
fails $ac lock_only_guard noReissueInsideTheCooldown

for m in email_codes reset_codes; do
  echo "models: $m keeps safety (quint run, 30 steps)"
  quint run $ac --main=$m --invariant safety --max-steps 30 --max-samples 10000 --verbosity 1
done
echo "models: refresh_rotation keeps safety (quint run, 30 steps)"
quint run $rr --invariant safety --max-steps 30 --max-samples 10000 --verbosity 1

echo "models: exporting traces to $traces"
rm -rf "$traces" && mkdir -p "$traces"
for m in email_codes reset_codes; do
  quint run $ac --main=$m --max-steps 20 --n-traces 300 --max-samples 300 \
    --out-itf "$traces/${m}_{seq}.itf.json" --verbosity 0
  # Random traces rarely reach a refused issue (the races task 4.1 fixed), so
  # add some that do: each is a counterexample to "no issue is refused". Any
  # other failure (a Quint error, no trace written) stops the run.
  for n in 1 2 3; do
    witness="$traces/${m}_witness${n}_0.itf.json"
    out=$(quint run $ac --main=$m --invariant noIssueIsRefused --max-steps 20 \
      --out-itf "$traces/${m}_witness${n}_{seq}.itf.json" --verbosity 1 2>&1) && status=0 || status=$?
    if ((status == 0)) || ! grep -q '^\[violation\]' <<<"$out" || [[ ! -s $witness ]]; then
      echo "$out" >&2
      echo "models: no witness trace in which $m refuses an issue" >&2
      exit 1
    fi
  done
done
quint run $rr --max-steps 20 --n-traces 300 --max-samples 300 \
  --out-itf "$traces/refresh_rotation_{seq}.itf.json" --verbosity 0

echo "models: replaying the traces against Postgres"
cd backend
MODEL_TRACES_DIR="$PWD/target/model-traces" cargo nextest run --test model_conformance --run-ignored only
