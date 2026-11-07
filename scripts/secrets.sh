#!/usr/bin/env bash
# SOPS + age secrets. deploy/secrets/access.yaml says who may read what; this
# renders .sops.yaml from it and keeps each encrypted file's recipients in step.
#
#   secrets.sh check          policy valid, .sops.yaml current, every secret encrypted
#                             to exactly its policy's recipients. Needs no keys (CI).
#   secrets.sh sync           render .sops.yaml, re-key files whose recipients changed
#   secrets.sh new <path>     encrypt stdin into a new secret (plaintext never on disk)
#   secrets.sh decrypt <env>  decrypt <env>'s secrets into deploy/compose/.secrets/ (hosts)
#
# Secrets live at deploy/secrets/<env>/<name>.sops.env (dotenv, encrypted per
# value) or <name>.sops (one opaque value). `sops edit <file>` edits one in place.
set -euo pipefail
cd "$(dirname "$0")/.."

dir=deploy/secrets
access=$dir/access.yaml
config=.sops.yaml
out=deploy/compose/.secrets

die() { echo "secrets: $*" >&2; exit 1; }
# yq prints an empty line for an empty result; drop blank lines from every query.
q() { yq -r "$1" "$access" | sed '/^$/d'; }

levels=()
load() {
  command -v yq >/dev/null || die "yq is required (mise install)"
  mapfile -t levels < <(q '.levels // [] | .[]')
  ((${#levels[@]})) || die "$access: levels is empty"
}

level_index() {
  local i
  for i in "${!levels[@]}"; do [[ ${levels[i]} == "$1" ]] && { echo "$i"; return 0; }; done
  return 1
}

people_tsv() { q '.people // {} | to_entries[] | [.key, .value.key, .value.level] | @tsv'; }
machines_tsv() { q '.machines // {} | to_entries[] | [.key, .value.key, .value.env] | @tsv'; }
names() { P=$1 L=$2 q '.secrets[strenv(P)][strenv(L)] // [] | .[]'; }

# Bech32 data charset: lowercase alphanumerics minus 1, b, i, o.
is_age_key() { [[ $1 =~ ^age1[02-9ac-hj-np-z]{58}$ ]]; }

validate() {
  local name key level path env e list n
  while IFS=$'\t' read -r name key level; do
    is_age_key "$key" || die "people.$name.key is not an age public key"
    level_index "$level" >/dev/null || die "people.$name.level '$level' is not one of: ${levels[*]}"
  done < <(people_tsv)
  while IFS=$'\t' read -r name key env; do
    is_age_key "$key" || die "machines.$name.key is not an age public key"
    level_index "$env" >/dev/null || die "machines.$name.env '$env' is not one of: ${levels[*]}"
  done < <(machines_tsv)
  while read -r path; do
    [[ $path =~ ^[^/]+/[^/]+\.sops(\.env)?$ ]] || die "secrets.$path: must be <level>/<name>.sops or <level>/<name>.sops.env"
    env=${path%%/*}
    e=$(level_index "$env") || die "secrets.$path: '$env' is not one of: ${levels[*]}"
    for list in only also; do
      while read -r n; do
        level=$(N=$n q '.people[strenv(N)].level // ""')
        [[ -n $level ]] || die "secrets.$path.$list: '$n' is not in people"
        if [[ $list == only ]] && (($(level_index "$level") < e)); then
          die "secrets.$path.only: '$n' is cleared for $level, below $env (use also: to widen)"
        fi
      done < <(names "$path" "$list")
    done
  done < <(q '.secrets // {} | keys | .[]')
}

# Recipient keys for a secret path ("<env>/<name>", or "<env>/" for the level default).
recipients() {
  local path=$1 env=${1%%/*} e name key level only also
  e=$(level_index "$env")
  mapfile -t only < <(names "$path" only)
  mapfile -t also < <(names "$path" also)
  {
    while IFS=$'\t' read -r name key level; do
      if ((${#only[@]})); then
        [[ " ${only[*]} " == *" $name "* ]] || continue
      else
        (($(level_index "$level") >= e)) || continue
      fi
      echo "$key"
    done < <(people_tsv)
    for name in "${also[@]}"; do N=$name q '.people[strenv(N)].key'; done
    while IFS=$'\t' read -r name key level; do
      if [[ $level == "$env" ]]; then echo "$key"; fi
    done < <(machines_tsv)
  } | sort -u
}

who() { # key -> name, for messages
  K=$1 q '[(.people // {}), (.machines // {})] | .[] | to_entries[] | select(.value.key == strenv(K)) | .key' | head -1 | grep . || echo "$1"
}

rule() { # regex, policy path
  local keys
  keys=$(recipients "$2" | paste -sd, -)
  [[ -n $keys ]] || die "nobody can read $2: add a person or machine to $access"
  printf "  - path_regex: '%s'\n    age: %s\n" "$1" "$keys"
}

render() {
  local path env
  echo "# GENERATED from $access by scripts/secrets.sh. Don't edit:"
  echo "# change the policy and run \`just secrets-sync\`."
  echo "creation_rules:"
  while read -r path; do
    rule "^$dir/${path//./\\.}\$" "$path"
  done < <(q '.secrets // {} | keys | .[]')
  for env in "${levels[@]}"; do rule "^$dir/$env/" "$env/"; done
}

store() { [[ $1 == *.sops.env ]] && echo dotenv || echo binary; }

file_recipients() {
  if [[ $(store "$1") == dotenv ]]; then
    sed -n 's/^sops_age__list_[0-9]*__map_recipient=//p' "$1"
  else
    yq -p json -r '.sops.age // [] | .[].recipient' "$1" 2>/dev/null || true
  fi | sort -u
}

secret_files() { find "$dir" -mindepth 1 -path "$dir/README.md" -prune -o -path "$access" -prune -o -print | sort; }

policy_path() { echo "${1#"$dir"/}"; }

cmd_check() {
  load
  validate
  local errors=0 f p have want
  if ! diff -u "$config" <(render) >&2; then
    echo "secrets: $config is out of date with $access: run \`just secrets-sync\`" >&2
    errors=1
  fi
  while read -r f; do
    p=$(policy_path "$f")
    if [[ -d $f ]]; then
      [[ $p != */* ]] && level_index "$p" >/dev/null && continue
      echo "secrets: $f: unexpected directory (levels: ${levels[*]})" >&2; errors=1; continue
    fi
    if ! [[ $p =~ ^[^/]+/[^/]+\.sops(\.env)?$ ]] || ! level_index "${p%%/*}" >/dev/null; then
      echo "secrets: $f: not a secret path (<level>/<name>.sops[.env]); plaintext must never live here" >&2
      errors=1; continue
    fi
    have=$(file_recipients "$f")
    if [[ -z $have ]]; then
      echo "secrets: $f: not SOPS-encrypted" >&2; errors=1; continue
    fi
    want=$(recipients "$p")
    if [[ $have != "$want" ]]; then
      echo "secrets: $f: recipients differ from policy: run \`just secrets-sync\`" >&2
      diff <(echo "$want") <(echo "$have") | sed -n 's/^< /  missing: /p; s/^> /  extra:   /p' >&2
      errors=1
    fi
  done < <(secret_files)
  ((errors == 0)) || exit 1
  echo "secrets: policy, $config and recipients agree"
}

cmd_sync() {
  load
  validate
  render >"$config"
  local failed=() f p have want removed
  while read -r f; do
    [[ -f $f ]] || continue
    p=$(policy_path "$f")
    have=$(file_recipients "$f")
    want=$(recipients "$p")
    [[ $have == "$want" ]] && continue
    removed=$(comm -23 <(echo "$have") <(echo "$want") | grep . || true)
    # updatekeys keeps the data key, so after a removal also rotate it: someone
    # who kept the old data key mustn't be able to read future edits.
    if sops updatekeys -y --input-type "$(store "$f")" "$f" &&
      { [[ -z $removed ]] || sops rotate -i --input-type "$(store "$f")" --output-type "$(store "$f")" "$f"; }; then
      echo "secrets: re-keyed $f"
      if [[ -n $removed ]]; then
        echo "secrets: $f: removed $(while read -r k; do who "$k"; done <<<"$removed" | paste -sd, -)." >&2
        echo "  They can still decrypt the old version from git history: rotate these values." >&2
      fi
    else
      failed+=("$f")
    fi
  done < <(secret_files)
  if ((${#failed[@]})); then
    echo "secrets: couldn't re-key (you can't decrypt them; ask someone who can to run \`just secrets-sync\`):" >&2
    printf '  %s\n' "${failed[@]}" >&2
    exit 1
  fi
  cmd_check
}

cmd_new() {
  local f=${1:?usage: secrets.sh new deploy/secrets/<level>/<name>.sops[.env] < plaintext}
  load
  validate
  local p type
  p=$(policy_path "$f")
  [[ $f == "$dir"/* && $p =~ ^[^/]+/[^/]+\.sops(\.env)?$ ]] || die "$f: must be $dir/<level>/<name>.sops or .sops.env"
  level_index "${p%%/*}" >/dev/null || die "$f: '${p%%/*}' is not one of: ${levels[*]}"
  [[ ! -e $f ]] || die "$f exists: edit it with \`sops edit $f\`"
  diff -q "$config" <(render) >/dev/null || die "$config is out of date: run \`just secrets-sync\` first"
  [[ -t 0 ]] && echo "Paste the value, then Ctrl-D:" >&2
  type=$(store "$f")
  mkdir -p "$(dirname "$f")"
  tmp=$(mktemp)
  trap 'rm -f "$tmp"' EXIT
  sops encrypt --filename-override "$f" --input-type "$type" --output-type "$type" /dev/stdin >"$tmp"
  mv "$tmp" "$f"
  trap - EXIT
  echo "secrets: wrote $f for: $(while read -r k; do who "$k"; done < <(file_recipients "$f") | paste -sd, -)"
}

cmd_decrypt() {
  local env=${1:?usage: secrets.sh decrypt <level>} f name target errors=0
  [[ -d $dir/$env ]] || die "no secrets for '$env' ($dir/$env)"
  # The directory keeps other host users out. Files are 0644 so containers
  # that run as non-root (Grafana is uid 472) can read their bind-mounted secret.
  mkdir -p "$out"
  chmod 700 "$out"
  for f in "$dir/$env"/*.sops "$dir/$env"/*.sops.env; do
    [[ -f $f ]] || continue
    name=$(basename "$f")
    case $name in
      *.sops.env) target=$out/${name%.sops.env}.env ;;
      *) target=$out/${name%.sops} ;;
    esac
    if sops decrypt --input-type "$(store "$f")" --output-type "$(store "$f")" "$f" >"$target.tmp"; then
      chmod 644 "$target.tmp"
      mv "$target.tmp" "$target"
      echo "secrets: $target"
    else
      rm -f "$target.tmp"
      echo "secrets: couldn't decrypt $f" >&2
      errors=1
    fi
  done
  ((errors == 0))
}

case ${1:-} in
  check | sync | new | decrypt) c=$1; shift; "cmd_$c" "$@" ;;
  *) sed -n '2,12s/^# \{0,1\}//p' "$0" >&2; exit 2 ;;
esac
