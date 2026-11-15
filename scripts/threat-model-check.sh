#!/usr/bin/env bash
# Checks docs/security/threat-model.md (OT-010): every property `### P-<nn>: …`
# names at least one check or carries a `- Gap:` line, property numbers are
# unique, and every named check still resolves:
#
#   `just <recipe>`          the recipe exists in the justfile
#   `path/file.rs::name`     the file exists and declares `fn name` at the start
#                            of a line (so a mention in a comment doesn't count)
#   `path/model.ext::name`   the file exists and contains `name` as a whole word
#                            (matched literally, not as a pattern)
#   `path/file`              the file exists
#
# Every indented entry under `- Checks:` must be one backticked check; anything
# else there fails, so a malformed entry can't be silently skipped.
#
# Resolution is by name only: it catches renames and deletions, not whether a
# test is meaningful. That's for review.
#
#   threat-model-check.sh [file]   default: docs/security/threat-model.md
set -euo pipefail

# Resolve a file argument against the caller's directory before moving to the
# repo root, where check paths are relative to.
doc=${1:-}
[[ -n $doc ]] && doc=$(realpath -- "$doc" 2>/dev/null || printf '%s' "$doc")
cd "$(dirname "$0")/.."
doc=${doc:-docs/security/threat-model.md}
[[ -f $doc ]] || { echo "threat-model-check: $doc not found" >&2; exit 1; }

errors=0
fail() {
  echo "threat-model-check: $*" >&2
  errors=$((errors + 1))
}

recipe_exists() {
  # A recipe line starts at column 0 with its name, then parameters or `:`.
  grep -Eq "^@?$1([[:space:]][^:=]*)?:([^=]|$)" justfile
}

resolve() {
  local id=$1 check=$2 path name
  if [[ $check == just\ * ]]; then
    name=${check#just }
    name=${name%% *}
    recipe_exists "$name" || fail "$id: no just recipe '$name'"
    return
  fi
  if [[ $check == *::* ]]; then
    path=${check%%::*}
    name=${check##*::}
  else
    path=$check
    name=
  fi
  if [[ ! -e $path ]]; then
    fail "$id: $path does not exist"
    return
  fi
  [[ -z $name ]] && return
  if [[ $path == *.rs ]]; then
    if [[ ! $name =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
      fail "$id: '$name' is not a Rust function name"
      return
    fi
    grep -Eq "^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?(const[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+${name}\b" "$path" ||
      fail "$id: no fn $name in $path"
  else
    grep -Fqw -- "$name" "$path" || fail "$id: '$name' not found in $path"
  fi
}

# Emits one record per property: `P <id>`, then `C <check>` per check, then
# `G` if it has a gap line. A property ends at the next `## ` or `### ` heading.
records=$(awk '
  /^### P-[0-9]+:/ { id = $2; sub(/:$/, "", id); print "P " id; inchecks = 0; next }
  /^##+ / { id = ""; inchecks = 0; next }
  id == "" { next }
  /^- Checks:[[:space:]]*$/ { inchecks = 1; next }
  /^- Checks:/ { print "B " $0; inchecks = 1; next }
  /^- Gap:/ { print "G"; inchecks = 0; next }
  /^- / { inchecks = 0; next }
  inchecks && /^[[:space:]]+- `[^`]+`[[:space:]]*$/ {
    match($0, /`[^`]+`/)
    print "C " substr($0, RSTART + 1, RLENGTH - 2)
    next
  }
  inchecks && /^[[:space:]]+- / { line = $0; sub(/^[[:space:]]+/, "", line); print "B " line; next }
' "$doc")

declare -A seen=()
id='' checks=0 gap=0 count=0
finish() {
  [[ -z $id ]] && return
  if ((checks == 0 && gap == 0)); then
    fail "$id: names no check and has no Gap line"
  fi
}

while IFS= read -r line; do
  case $line in
    'P '*)
      finish
      id=${line#P }
      checks=0 gap=0 count=$((count + 1))
      [[ -n ${seen[$id]:-} ]] && fail "$id: number used twice"
      seen[$id]=1
      ;;
    'C '*)
      checks=$((checks + 1))
      resolve "$id" "${line#C }"
      ;;
    'B '*) fail "$id: malformed check entry (want one backticked check per line): ${line#B }" ;;
    G) gap=1 ;;
  esac
done <<< "$records"
finish

((count > 0)) || fail "no properties found in $doc"

if ((errors > 0)); then
  echo "threat-model-check: $errors problem(s) in $doc" >&2
  exit 1
fi
echo "threat-model-check: $count properties, all checks resolve"
