#!/usr/bin/env bash
# Formal coverage gate (OT-013 design D13), run by `just formal-coverage` and CI.
#
# docs/formal-coverage.yaml names, for every scenario of every requirement in
# scope, the checks that cover it, plus the threat model's P-35. This script
# fails when:
#
#   - a requirement or scenario in the specs has no entry, or an entry names
#     one the specs no longer have (a rename counts as both);
#   - a named check doesn't exist or wouldn't run (below);
#   - a requirement has no formal check (kani, invariant, broken, replay), no
#     pending scenario and no `reason:` saying why no proof or model applies;
#   - a scenario is `pending: <task>` and that task is ticked in tasks.md, or
#     any scenario is pending with `--final`. A scenario may give both checks
#     (what covers it now) and pending (the task that still owes a check);
#   - a scenario's `exception: <reason>` (no automated check exists, and why)
#     is empty or sits beside checks or pending. Exceptions pass `--final`;
#   - the manifest has a key it doesn't know, so a typo can't skip a check.
#
# Which specs: while the change is active, its spec deltas
# (openspec/changes/<change>/specs, ADDED and MODIFIED requirements), and every
# requirement there must be in the manifest. Once archived, openspec/specs, and
# only the manifest's requirements are demanded (other features share them).
#
# A check exists and runs when:
#
#   kani: file.rs::fn       `fn` carries #[kani::proof], and its doc comment
#                           states its bounds or assumptions
#   rust: file.rs::fn       `fn` carries a test attribute and no #[ignore]
#                           (nor a cfg_attr that ignores it)
#   replay: file.rs::fn     as `rust`, but #[ignore] is allowed when
#                           scripts/models.sh runs that test target
#   web: file.ts::title     an `it`/`test` call (Vitest, Playwright or
#                           node:test, so .ts or .mjs) with exactly that title, not
#                           skipped, todo or conditional, in a file without a
#                           skipped `describe` or a conditional skip
#                           (`test.skip(cond)`, `test.fixme()`, `ctx.skip()`),
#                           and not under tests/integration or tests/smoke,
#                           which `just ci` doesn't run
#   invariant: file.qnt::name  `val`/`def name` in the model, and a non-comment
#                           line of scripts/models.sh names it
#   broken: file.qnt::module   `module name` in the model, and a non-comment
#                           line of scripts/models.sh expects it to fail
#   just: recipe            the recipe exists in the justfile
#   shell: file.sh::fn      `fn()` is defined in the file and called there
#
# Like threat-model-check.sh, resolution is by name: it catches renames,
# deletions and switched-off tests, not whether a test is meaningful.
#
#   formal-coverage.sh [--final] [manifest]   default: docs/formal-coverage.yaml
#
# FORMAL_COVERAGE_ROOT overrides the repository root (for the fixture tests).
set -euo pipefail

final=0
manifest=
for arg in "$@"; do
  case $arg in
    --final) final=1 ;;
    -*) echo "formal-coverage: unknown option $arg" >&2; exit 2 ;;
    *) manifest=$(realpath -- "$arg" 2>/dev/null || printf '%s' "$arg") ;;
  esac
done
cd "${FORMAL_COVERAGE_ROOT:-$(dirname "$0")/..}"
manifest=${manifest:-docs/formal-coverage.yaml}
command -v yq >/dev/null || { echo "formal-coverage: yq is required (mise install)" >&2; exit 1; }
[[ -f $manifest ]] || { echo "formal-coverage: $manifest not found" >&2; exit 1; }

errors=0
fail() {
  echo "formal-coverage: $*" >&2
  errors=$((errors + 1))
}
kinds=" kani rust replay web invariant broken just shell "
formal_kinds=" kani invariant broken replay "

# ── Manifest ─────────────────────────────────────────────────────────────────────

# One yq pass: every leaf (and empty map or list) as `path\ttag\tvalue`, with
# the path's keys joined by U+001F. The shape is checked from these below.
sep=$'\x1f'
leaves=$(SEP=$sep yq -r '
  .. | select((tag != "!!map" and tag != "!!seq") or length == 0)
  | (path | map(tostring) | join(strenv(SEP))) + "\t" + tag + "\t" + (tostring | sub("[\t\n]"; " "))
' "$manifest") || { echo "formal-coverage: $manifest isn't valid YAML" >&2; exit 1; }
[[ $(yq -r 'tag' "$manifest") == '!!map' ]] || { echo "formal-coverage: $manifest is not a map" >&2; exit 1; }

change=
reqs=()            # "cap<sep>req", in manifest order
declare -A reason  # "cap<sep>req" -> why no proof or model applies
declare -A scens   # "cap<sep>req" -> its scenarios, one per line
declare -A pending # "cap<sep>req<sep>scen" -> task
declare -A exception # "cap<sep>req<sep>scen" -> why no automated check exists
declare -A checks  # "cap<sep>req<sep>scen" or "P-nn" -> "index\tkind\ttarget" lines
declare -A shapes  # "owner<sep>index" -> how many keys that check has
props=()
add_req() { [[ -v "scens[$1]" ]] || { reqs+=("$1"); scens[$1]=; }; }
add_scen() { # req-key scen
  [[ $'\n'${scens[$1]} == *$'\n'"$2"$'\n'* ]] || scens[$1]+="$2"$'\n'
}
add_check() { # owner label rest-of-path... (index kind) tag value
  local owner=$1 label=$2 index=${3:-} kind=${4:-} tag=$5 value=$6
  if [[ -z $index ]]; then
    if [[ $tag == '!!seq' ]]; then fail "$label: no checks"; else fail "$label: checks must be a list"; fi
  elif [[ -z $kind ]]; then
    fail "$label: check $((index + 1)) must be one 'kind: target'"
  else
    checks[$owner]+="$index"$'\t'"$kind"$'\t'"$value"$'\n'
    shapes[$owner$sep$index]=$((${shapes[$owner$sep$index]:-0} + 1))
  fi
}

while IFS=$'\t' read -r path tag value; do
  [[ -n $path ]] || continue
  IFS=$sep read -r -a seg <<<"$path"
  n=${#seg[@]}
  case ${seg[0]} in
    change) change=$value ;;
    capabilities)
      if ((n < 3)); then fail "capabilities${seg[1]:+ / ${seg[1]}}: must map requirements to entries"; continue; fi
      rk=${seg[1]}$sep${seg[2]}
      add_req "$rk"
      label="${seg[1]} / ${seg[2]}"
      if ((n == 3)); then [[ $tag == '!!map' ]] || fail "$label: must be a map"; continue; fi
      case ${seg[3]} in
        reason) if ((n == 4)); then reason[$rk]=$value; else fail "$label: reason must be text"; fi ;;
        scenarios)
          if ((n == 4)); then [[ $tag == '!!map' ]] || fail "$label: scenarios must be a map"; continue; fi
          sk=$rk$sep${seg[4]}
          add_scen "$rk" "${seg[4]}"
          label+=" / ${seg[4]}"
          if ((n == 5)); then [[ $tag == '!!map' ]] || fail "$label: must be a map"; continue; fi
          case ${seg[5]} in
            pending)
              if ((n == 6)) && [[ $tag == '!!str' ]]; then
                pending[$sk]=$value
              else
                # Unquoted, 7.10 would be read as the number 7.1.
                fail "$label: pending must be a quoted task number, as '7.2'"
                pending[$sk]=
              fi
              ;;
            exception)
              if ((n == 6)) && [[ $tag == '!!str' && $value =~ [^[:space:]] ]]; then
                exception[$sk]=$value
              else
                fail "$label: exception must say why no automated check exists"
                exception[$sk]=
              fi
              ;;
            checks)
              if ((n <= 8)); then
                add_check "$sk" "$label" "${seg[6]:-}" "${seg[7]:-}" "$tag" "$value"
              else
                fail "$label: check $((seg[6] + 1)) must be one 'kind: target'"
              fi
              ;;
            *) fail "$label: unknown key '${seg[5]}'" ;;
          esac
          ;;
        *) fail "$label: unknown key '${seg[3]}'" ;;
      esac
      ;;
    properties)
      if ((n < 2)); then fail "properties: must map properties to checks"; continue; fi
      [[ " ${props[*]} " == *" ${seg[1]} "* ]] || props+=("${seg[1]}")
      if ((n == 2)); then fail "${seg[1]}: must be a map"; continue; fi
      [[ ${seg[2]} == checks ]] || { fail "${seg[1]}: unknown key '${seg[2]}'"; continue; }
      if ((n <= 5)); then
        add_check "${seg[1]}" "${seg[1]}" "${seg[3]:-}" "${seg[4]:-}" "$tag" "$value"
      else
        fail "${seg[1]}: check $((seg[3] + 1)) must be one 'kind: target'"
      fi
      ;;
    *) fail "unknown top-level key '${seg[0]}'" ;;
  esac
done <<<"$leaves"
[[ -n $change ]] || { echo "formal-coverage: $manifest names no change" >&2; exit 1; }

# The specs and tasks to read: the active change, or its archive.
if [[ -d openspec/changes/$change ]]; then
  mode="delta"
  tasks=openspec/changes/$change/tasks.md
else
  archived=$(compgen -G "openspec/changes/archive/*-$change" | tail -1 || true)
  [[ -n $archived ]] || { echo "formal-coverage: change '$change' is neither active nor archived" >&2; exit 1; }
  mode=archived
  tasks=$archived/tasks.md
fi

# Every requirement and scenario, as `REQ\tcap\treq` and `SCEN\tcap\treq\tscen`.
spec_entries() { # cap file mode
  awk -v cap="$1" -v mode="$3" '
    function trim(s) { sub(/[ \t\r]+$/, "", s); return s }
    /^```/ { fence = !fence; next }
    fence { next }
    /^## / {
      if (mode == "delta") inside = ($0 ~ /^## (ADDED|MODIFIED) Requirements/)
      else inside = ($0 ~ /^## Requirements/)
      req = ""
      next
    }
    inside && /^### Requirement: / { req = trim(substr($0, 18)); print "REQ\t" cap "\t" req; next }
    inside && /^### / { req = ""; next }
    inside && req != "" && /^#### Scenario: / { print "SCEN\t" cap "\t" req "\t" trim(substr($0, 16)) }
  ' "$2"
}

specs=$(mktemp)
trap 'rm -f "$specs"' EXIT
if [[ $mode == delta ]]; then
  for file in openspec/changes/"$change"/specs/*/spec.md; do
    [[ -f $file ]] || continue
    cap=${file%/spec.md}
    spec_entries "${cap##*/}" "$file" delta >>"$specs"
  done
else
  while IFS= read -r cap; do
    file=openspec/specs/$cap/spec.md
    if [[ -f $file ]]; then spec_entries "$cap" "$file" main >>"$specs"; else fail "$cap: no $file"; fi
  done < <(for rk in "${reqs[@]}"; do printf '%s\n' "${rk%%"$sep"*}"; done | sort -u)
fi

# ── Checks ───────────────────────────────────────────────────────────────────────

# The attribute and doc lines directly above `fn name` in a Rust file, or
# nothing when there is no such fn. Stops at a blank line or one ending a
# block or statement, so it never reaches the item before.
rust_preamble() { # file name
  awk -v name="$2" '
    { line[NR] = $0 }
    $0 ~ "^[ \t]*(pub(\\([a-z]+\\))? )?(const )?(async )?(unsafe )?fn " name "[ \t]*[(<]" { found = NR; exit }
    END {
      if (!found) exit 1
      start = found
      for (i = found - 1; i > 0; i--) {
        l = line[i]
        if (l ~ /^[ \t]*$/) break
        if (l !~ /^[ \t]*\/\// && l ~ /[;{}][ \t]*$/) break
        start = i
      }
      for (i = start; i <= found; i++) print line[i]
    }
  ' "$1"
}

models_lines() { # non-comment lines of scripts/models.sh
  [[ -f scripts/models.sh ]] && grep -v '^[[:space:]]*#' scripts/models.sh || true
}
has_word() { grep -Eq "(^|[^A-Za-z0-9_])$1([^A-Za-z0-9_]|\$)"; }

check_rust() { # kind file name
  local kind=$1 file=$2 name=$3 pre
  pre=$(rust_preamble "$file" "$name") || { echo "no fn $name in $file"; return; }
  if [[ $kind == kani ]]; then
    grep -q '#\[kani::proof\]' <<<"$pre" || { echo "$file::$name has no #[kani::proof]"; return; }
    grep -Eiq '^[[:space:]]*///.*(bound|assum)' <<<"$pre" ||
      echo "$file::$name doesn't state its bounds or assumptions in its doc comment"
    return
  fi
  grep -Eq '#\[(test|tokio::test|sqlx::test|rstest)([](]|$)' <<<"$pre" ||
    { echo "$file::$name isn't a test"; return; }
  # `#[ignore]`, or `#[cfg_attr(<cond>, ignore)]` (possibly across lines).
  if grep -q '#\[ignore' <<<"$pre" || tr '\n' ' ' <<<"$pre" | grep -Eq 'cfg_attr\([^]]*\bignore\b'; then
    if [[ $kind != replay ]]; then
      echo "$file::$name is ignored"
    elif ! models_lines | grep -Eq -- "--test[[:space:]]+$(basename "$file" .rs)([[:space:]]|\$)"; then
      echo "$file::$name is ignored and scripts/models.sh doesn't run $(basename "$file" .rs)"
    fi
  fi
}

check_web() { # file title
  local file=$1 title=$2 n line prev q found=0
  # Playwright's default config ignores these; neither `just ci` nor CI runs them.
  if [[ $file == */tests/integration/* || $file == */tests/smoke/* || $file == tests/integration/* || $file == tests/smoke/* ]]; then
    echo "$file isn't run by \`just ci\` (integration and smoke flows need a live stack)"
    return
  fi
  if grep -Eq '(describe)\.(skip|todo|skipIf|runIf|fixme)\b|describe\.only' "$file"; then
    echo "$file has a skipped describe"
    return
  fi
  # A conditional skip (`test.skip(cond)`, `test.fixme()`, `ctx.skip()`) may sit
  # in a describe or a test body; its scope isn't parsed, so any fails the file.
  # (`test.skip('title', …)` declares another, skipped test and is fine.)
  if grep -Eq "(^|[^A-Za-z0-9_.])(it|test)\.(skip|fixme)\([[:space:]]*[^'\"\`[:space:]]|[A-Za-z_]\.skip\(\)" "$file"; then
    echo "$file has a conditional skip"
    return
  fi
  while IFS=: read -r n line; do
    prev=$( ((n > 1)) && sed -n "$((n - 1))p" "$file" || true)
    for q in "'" '"' '`'; do
      [[ $line == *"$q$title$q"* ]] || continue
      local call="${line%%"$q$title$q"*}"
      [[ $call =~ (^|[^A-Za-z0-9_.])(it|test)(\.[A-Za-z]+)*\([[:space:]]*$ ]] ||
        { [[ $call =~ ^[[:space:]]*$ && $prev =~ (^|[^A-Za-z0-9_.])(it|test)(\.[A-Za-z]+)*\([[:space:]]*$ ]] || continue; }
      found=1
      local modifier=${BASH_REMATCH[3]:-}
      if [[ $modifier =~ \.(skip|todo|fails|skipIf|runIf|fixme|only)$ ]]; then
        echo "$file: '$title' is ${modifier#.}"
        return
      fi
    done
  done < <(grep -nF -- "$title" "$file")
  ((found)) || echo "no test '$title' in $file"
}

check_model() { # kind file name
  local kind=$1 file=$2 name=$3
  if [[ $kind == invariant ]]; then
    grep -Eq "^[[:space:]]*(pure[[:space:]]+)?(val|def)[[:space:]]+$name\b" "$file" ||
      { echo "no val or def $name in $file"; return; }
    models_lines | grep -qF "$file" || { echo "scripts/models.sh never mentions $file"; return; }
    models_lines | has_word "$name" || echo "scripts/models.sh doesn't check $name"
  else
    grep -Eq "^[[:space:]]*module[[:space:]]+$name\b" "$file" || { echo "no module $name in $file"; return; }
    models_lines | grep -E '(^|[[:space:]])fails[[:space:]]' | has_word "$name" ||
      echo "scripts/models.sh doesn't expect $name to fail"
  fi
}

check_shell() { # file name
  local file=$1 name=$2
  grep -Eq "^[[:space:]]*(function[[:space:]]+)?${name}[[:space:]]*\(\)" "$file" || { echo "no function $name in $file"; return; }
  (($(grep -v '^[[:space:]]*#' "$file" | has_word_count "$name") >= 2)) && return
  # A runner that calls every test_ function it finds (`declare -F`) counts.
  [[ $name == test_* ]] && grep -v '^[[:space:]]*#' "$file" | grep -q 'declare -F' &&
    grep -v '^[[:space:]]*#' "$file" | grep -qF "'^test_'" && return
  echo "$file never calls $name"
}
has_word_count() { grep -Ec "(^|[^A-Za-z0-9_])$1([^A-Za-z0-9_]|\$)" || true; }

recipe_exists() { grep -Eq "^@?$1([[:space:]][^:=]*)?:([^=]|$)" justfile; }

# Prints why the check doesn't hold, or nothing.
check() { # kind target
  local kind=$1 target=$2 file name
  if [[ $kind == just ]]; then
    recipe_exists "$target" || echo "no just recipe '$target'"
    return
  fi
  [[ $target == *::* ]] || { echo "'$target' isn't file::name"; return; }
  file=${target%%::*}
  name=${target#*::}
  [[ -n $name ]] || { echo "'$target' names nothing"; return; }
  [[ -f $file ]] || { echo "no file $file"; return; }
  case $kind in
    kani | rust | replay) check_rust "$kind" "$file" "$name" ;;
    web) check_web "$file" "$name" ;;
    invariant | broken) check_model "$kind" "$file" "$name" ;;
    shell) check_shell "$file" "$name" ;;
  esac
}

run_checks() { # owner label: runs each of its checks; sets formal=1 on a formal one
  local owner=$1 label=$2 index kind target why
  while IFS=$'\t' read -r index kind target; do
    [[ -n $index ]] || continue
    if ((${shapes[$owner$sep$index]} != 1)); then
      fail "$label: check $((index + 1)) must be one 'kind: target'"
      continue
    fi
    if [[ $kinds != *" $kind "* ]]; then fail "$label: unknown check kind '$kind'"; continue; fi
    # A check that errors out must fail, not pass with nothing said.
    why=$(check "$kind" "$target" 2>&1) || why="checking $kind $target failed: ${why:-no output}"
    [[ -z $why ]] || fail "$label: $why"
    [[ $formal_kinds == *" $kind "* ]] && formal=1
  done <<<"${checks[$owner]:-}"
}

task_state() { # task id: prints ticked, unticked or missing
  local line
  line=$(grep -E "^[[:space:]]*- \[[ xX]\] ${1//./\\.}([[:space:]]|$)" "$tasks" 2>/dev/null | head -1 || true)
  if [[ -z $line ]]; then echo missing
  elif [[ $line == *'- [ ]'* ]]; then echo unticked
  else echo ticked
  fi
}

# ── Walk the manifest ────────────────────────────────────────────────────────────

in_specs() { grep -qxF "$1" "$specs"; }
pending_total=0
scenario_total=0
exception_total=0
for rk in "${reqs[@]}"; do
  cap=${rk%%"$sep"*}
  req=${rk#*"$sep"}
  in_specs "REQ"$'\t'"$cap"$'\t'"$req" || fail "$cap / $req: no such requirement in the specs (renamed or removed?)"
  formal=0
  pending_here=0
  while IFS= read -r scen; do
    [[ -n $scen ]] || continue
    sk=$rk$sep$scen
    label="$cap / $req / $scen"
    scenario_total=$((scenario_total + 1))
    in_specs "SCEN"$'\t'"$cap"$'\t'"$req"$'\t'"$scen" || fail "$label: no such scenario in the specs (renamed or removed?)"
    if [[ -v "exception[$sk]" ]]; then
      # An exception stands alone: a scenario with a check needs none, and one
      # still owed a check isn't an exception yet.
      if [[ -v "pending[$sk]" || -v "checks[$sk]" ]]; then
        fail "$label: an exception can't sit beside checks or pending"
      fi
      exception_total=$((exception_total + 1))
      continue
    fi
    if [[ ! -v "pending[$sk]" && ! -v "checks[$sk]" ]]; then
      fail "$label: give checks, pending, or both (or an exception)"
      continue
    fi
    # Both: the checks that exist now, and the task that still owes one (a
    # model, say). The checks are run either way.
    [[ -v "checks[$sk]" ]] && run_checks "$sk" "$label"
    if [[ -v "pending[$sk]" ]]; then
      task=${pending[$sk]}
      pending_total=$((pending_total + 1))
      pending_here=1
      if ((final)); then
        fail "$label: still pending ($task), and --final allows none"
      else
        case $(task_state "$task") in
          ticked) fail "$label: pending $task, but $task is ticked" ;;
          missing) fail "$label: pending $task, but $tasks has no task $task" ;;
        esac
      fi
    fi
  done <<<"${scens[$rk]}"
  if ((!formal && !pending_here)) && [[ -z ${reason[$rk]:-} ]]; then
    fail "$cap / $req: no proof, model or replay, and no reason why none applies"
  fi
done

# Everything in the specs needs an entry (all of the delta; the listed
# requirements' scenarios once archived).
while IFS=$'\t' read -r tag cap req scen; do
  rk=$cap$sep$req
  if [[ $tag == REQ ]]; then
    [[ $mode == delta && ! -v "scens[$rk]" ]] && fail "$cap / $req: requirement has no entry"
  elif [[ -v "scens[$rk]" && $'\n'${scens[$rk]} != *$'\n'"$scen"$'\n'* ]]; then
    fail "$cap / $req / $scen: scenario has no entry"
  fi
done <"$specs"
[[ -s $specs ]] || fail "no requirements found in the $mode specs"

# Threat model properties: the property exists and its checks hold.
for prop in "${props[@]}"; do
  grep -Eq "^### $prop:" docs/security/threat-model.md 2>/dev/null || fail "$prop: not in docs/security/threat-model.md"
  [[ -v "checks[$prop]" ]] || { fail "$prop: no checks"; continue; }
  run_checks "$prop" "$prop"
done

if ((errors > 0)); then
  echo "formal-coverage: $errors problem(s) in $manifest" >&2
  exit 1
fi
echo "formal-coverage: ok ($mode specs, $scenario_total scenarios, $pending_total pending, $exception_total exceptions)"
