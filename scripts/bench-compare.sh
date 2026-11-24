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
# Tunables: BENCH_ROUNDS, BENCH_THRESHOLD (percent, default 10), CRITERION_ARGS,
#   BENCH_ALLOW_BROKEN_BASE (set to 1 to warn instead of failing when the base produces no results).
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
    local args=() bench_dir found=0
    if [ -d "$1/backend/crates/server/benches" ]; then
      bench_dir="$1/backend/crates/server/benches"
      args+=(-p server)
    else
      bench_dir="$1/backend/benches"
    fi
    for b in "${benches[@]}"; do
      if [ -f "$bench_dir/$b.rs" ]; then
        args+=(--bench "$b")
        found=1
      fi
    done
    if [ "$found" = 0 ]; then echo "no backend benches in $2; skipping"; return; fi
    case $2 in
      bench-base-*) args+=(--no-fail-fast) ;;
    esac
    echo "── backend benches: $2"
    # shellcheck disable=SC2086 # criterion_args is a flag list.
    if ! (cd "$1/backend" && cargo bench --locked "${args[@]}" -- --save-baseline "$2" $criterion_args); then
      # `--no-fail-fast` lets other benches run past runtime failures (not
      # compile errors). If the base produced at least some results, warn and
      # compare what finished. If it produced no results at all (e.g. a compile
      # error on base), fail by default so every PR bench isn't silently marked
      # "new" — set BENCH_ALLOW_BROKEN_BASE=1 when intentionally landing a PR
      # that repairs an uncompilable base. The PR side must still pass.
      case $2 in
        bench-base-*)
          if ! { [ -d "$CRITERION_HOME" ] && find "$CRITERION_HOME" -type f -path "*/$2/estimates.json" | grep -q .; }; then
            if [ "${BENCH_ALLOW_BROKEN_BASE:-0}" = 1 ]; then
              echo "::warning::backend benches failed on the base side ($2) with no results (allowed via BENCH_ALLOW_BROKEN_BASE=1)"
            else
              echo "::error::backend benches failed on the base side ($2) and produced no results (set BENCH_ALLOW_BROKEN_BASE=1 to override)" >&2
              return 1
            fi
          else
            echo "::warning::backend benches failed on the base side ($2); comparing the benches that finished"
          fi
          ;;
        *) return 1 ;;
      esac
    fi
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
    # Vitest 5 dropped --outputJson for the JSON reporter; the base side may
    # still be on 4, so pick the flags per checkout.
    local major out_flags
    major=$(cd "$1/apps/web" && node -p "require('vitest/package.json').version.split('.')[0]")
    if [ "$major" -ge 5 ]; then out_flags=(--reporter=json --outputFile="$2"); else out_flags=(--outputJson "$2"); fi
    (cd "$1" && pnpm --filter @overkill-todo/web exec vitest bench --run "${out_flags[@]}")
  }
  (cd "$worktree" && pnpm install --frozen-lockfile --silent \
    && pnpm --filter @overkill-todo/web exec playwright install chromium >/dev/null)
  for r in $(seq "$rounds"); do
    run_web "$worktree" "$out/web-base-$r.json"
    run_web "$root" "$out/web-pr-$r.json"
  done
fi

node scripts/bench-report.mjs "$out" "$(git rev-parse --short "$base_sha")"
