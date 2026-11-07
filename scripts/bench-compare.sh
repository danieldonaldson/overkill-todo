#!/usr/bin/env bash
# Benchmark a base ref and the current working tree back to back on this
# machine, then report the per-benchmark change (scripts/bench-report.mjs).
# Same machine, minutes apart, so shared-runner noise mostly cancels out.
#
# Usage: bench-compare.sh [--backend] [--web] [base-ref]
#   Defaults: both suites, base main.
#
# Report-only: exits 0 even on regressions unless BENCH_FAIL_ON_REGRESSION=1.
# Sides alternate for BENCH_ROUNDS rounds (default 2: base, PR, base, PR) and
# the report keeps each side's fastest run, since noise only ever adds time.
# Tunables: BENCH_ROUNDS, BENCH_THRESHOLD (percent, default 10), CRITERION_ARGS.
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD

backend=0 web=0 base=main
for arg in "$@"; do
  case $arg in
    --backend) backend=1 ;;
    --web) web=1 ;;
    -*) echo "unknown flag: $arg" >&2; exit 2 ;;
    *) base=$arg ;;
  esac
done
[ $backend = 1 ] || [ $web = 1 ] || { backend=1; web=1; }
base_sha=$(git rev-parse --verify "$base^{commit}")

# DB-free benches only: query latency is too noisy for an A/B verdict.
benches=(crypto domain middleware)
# Short runs keep the job to a few minutes; base and PR use identical settings,
# so the comparison stays fair.
criterion_args=${CRITERION_ARGS:---warm-up-time 1 --measurement-time 2 --noplot}
rounds=${BENCH_ROUNDS:-2}

out="$root/backend/target/bench-compare"
rm -rf "$out" && mkdir -p "$out"
worktree=$(mktemp -d)/base
cleanup() { git -C "$root" worktree remove --force "$worktree" 2>/dev/null || true; }
trap cleanup EXIT
git worktree add --quiet --detach "$worktree" "$base_sha"

if [ $backend = 1 ]; then
  # One toolchain for both sides (the PR's pin), so only the code differs.
  export RUSTUP_TOOLCHAIN=${RUSTUP_TOOLCHAIN:-$(sed -n 's/^channel *= *"\(.*\)"/\1/p' backend/rust-toolchain.toml)}
  # Shared target dir reuses compiled deps; each side's crate builds separately
  # (cargo keys artifacts by source path). Both sides write into one criterion
  # home under distinct baseline names.
  export CARGO_TARGET_DIR="$root/backend/target" SQLX_OFFLINE=true
  export CRITERION_HOME="$out/criterion"

  run_side() { # <checkout> <baseline-name>
    local args=()
    for b in "${benches[@]}"; do [ -f "$1/backend/benches/$b.rs" ] && args+=(--bench "$b"); done
    if [ ${#args[@]} = 0 ]; then echo "no backend benches in $2; skipping"; return; fi
    echo "── backend benches: $2"
    # shellcheck disable=SC2086 # criterion_args is a flag list.
    (cd "$1/backend" && cargo bench --locked "${args[@]}" -- --save-baseline "$2" $criterion_args)
  }
  for r in $(seq "$rounds"); do
    run_side "$worktree" "bench-base-$r"
    run_side "$root" "bench-pr-$r"
  done
fi

if [ $web = 1 ]; then
  run_web() { # <checkout> <output-json>
    if ! find "$1/apps/web/src" -name '*.bench.ts' | grep -q .; then
      echo "no web benches for $(basename "$2" .json); skipping"; return
    fi
    echo "── web benches: $(basename "$2" .json)"
    (cd "$1" && pnpm --filter @overkill-todo/web exec vitest bench --run --outputJson "$2")
  }
  (cd "$worktree" && pnpm install --frozen-lockfile --silent \
    && pnpm --filter @overkill-todo/web exec playwright install chromium >/dev/null)
  for r in $(seq "$rounds"); do
    run_web "$worktree" "$out/web-base-$r.json"
    run_web "$root" "$out/web-pr-$r.json"
  done
fi

node scripts/bench-report.mjs "$out" "$(git rev-parse --short "$base_sha")"
