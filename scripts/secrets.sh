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
# value) or <name>.sops (one opaque value), or <name>.sops.yaml (a SopsSecret the
# cluster's operator decrypts; only data/stringData values are encrypted).
# `sops edit <file>` edits one in place.
set -euo pipefail
cd "$(dirname "$0")/.."

dir=deploy/secrets
access=$dir/access.yaml
config=.sops.yaml
out=deploy/compose/.secrets

# <level>/<name>.sops, .sops.env or .sops.yaml
is_secret_path() { [[ $1 =~ ^[^/]+/[^/]+\.sops(\.env|\.yaml)?$ ]]; }
# The values SOPS encrypts in a SopsSecret. Names stay readable for review.
yaml_regex='^(data|stringData)$'

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

# Bech32 data charset: lowercase alphanumerics minus 1, b, i, o. Only hybrid
# post-quantum keys (ML-KEM-768 + X25519, age1pq1…) are accepted: secrets live
# in git forever, so a classical X25519 key (age1…) could be broken later by a
# quantum computer and every secret ever encrypted to it read from history.
is_age_key() { [[ $1 =~ ^age1pq1[02-9ac-hj-np-z]{1952}$ ]]; }
is_classical_age_key() { [[ $1 =~ ^age1[02-9ac-hj-np-z]{58}$ ]]; }
PQ_HINT="use a post-quantum key: \`age-keygen -pq\`, then its age1pq1… public key"

check_key() { # label, key
  is_classical_age_key "$2" && die "$1 is a classical age key (X25519), which isn't allowed; $PQ_HINT"
  is_age_key "$2" || die "$1 is not a post-quantum age public key; $PQ_HINT"
}

validate() {
  local name key level path env e list n
  while IFS=$'\t' read -r name key level; do
    check_key "people.$name.key" "$key"
    level_index "$level" >/dev/null || die "people.$name.level '$level' is not one of: ${levels[*]}"
  done < <(people_tsv)
  while IFS=$'\t' read -r name key env; do
    check_key "machines.$name.key" "$key"
    level_index "$env" >/dev/null || die "machines.$name.env '$env' is not one of: ${levels[*]}"
  done < <(machines_tsv)
  while read -r path; do
    is_secret_path "$path" || die "secrets.$path: must be <level>/<name>.sops, .sops.env or .sops.yaml"
    case $(P=$path q '.secrets[strenv(P)] | select(has("machines")) | .machines') in
      '' | false) ;;
      *) die "secrets.$path.machines: only 'false' is allowed (leave it out to include the environment's machines)" ;;
    esac
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

# `machines: false`: people only, for a secret its own environment's machines
# must not read (the cluster's Talos secrets, OT-014 D4).
no_machines() { [[ $(P=$1 q '.secrets[strenv(P)].machines') == false ]]; }

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
    if ! no_machines "$path"; then
      while IFS=$'\t' read -r name key level; do
        if [[ $level == "$env" ]]; then echo "$key"; fi
      done < <(machines_tsv)
    fi
  } | sort -u
}

who() { # key -> name, for messages
  K=$1 q '[(.people // {}), (.machines // {})] | .[] | to_entries[] | select(.value.key == strenv(K)) | .key' | head -1 | grep . || echo "$1"
}

rule() { # regex, policy path, [encrypted_regex]
  local keys
  keys=$(recipients "$2" | paste -sd, -)
  [[ -n $keys ]] || die "nobody can read $2: add a person or machine to $access"
  printf "  - path_regex: '%s'\n    age: %s\n" "$1" "$keys"
  [[ -z ${3:-} ]] || printf "    encrypted_regex: '%s'\n" "$3"
}

render() {
  local path env
  echo "# GENERATED from $access by scripts/secrets.sh. Don't edit:"
  echo "# change the policy and run \`just secrets-sync\`."
  echo "creation_rules:"
  while read -r path; do
    if [[ $path == *.sops.yaml ]]; then
      rule "^$dir/${path//./\\.}\$" "$path" "$yaml_regex"
    else
      rule "^$dir/${path//./\\.}\$" "$path"
    fi
  done < <(q '.secrets // {} | keys | .[]')
  # First match wins, so each level's SopsSecret rule precedes its general one.
  for env in "${levels[@]}"; do
    rule "^$dir/$env/[^/]+\\.sops\\.yaml\$" "$env/" "$yaml_regex"
    rule "^$dir/$env/" "$env/"
  done
}

store() {
  case $1 in
    *.sops.env) echo dotenv ;;
    *.sops.yaml) echo yaml ;;
    *) echo binary ;;
  esac
}

file_recipients() {
  if [[ $(store "$1") == dotenv ]]; then
    sed -n 's/^sops_age__list_[0-9]*__map_recipient=//p' "$1"
  elif [[ $(store "$1") == yaml ]]; then
    yq -r '.sops.age // [] | .[].recipient' "$1" 2>/dev/null || true
  else
    yq -p json -r '.sops.age // [] | .[].recipient' "$1" 2>/dev/null || true
  fi | sort -u
}

# A .sops.yaml must be one SopsSecret with a namespace and nothing in its
# templates but encrypted data/stringData and the fields that hold no values.
# Prints what's wrong, if anything. Needs no keys.
sops_secret_problems() {
  local f=$1
  [[ $(yq -N 'document_index' "$f" 2>/dev/null | wc -l) == 1 ]] || { echo "must be a single YAML document"; return; }
  [[ $(yq '.apiVersion + "/" + .kind' "$f") == isindir.github.com/v1alpha3/SopsSecret ]] ||
    { echo "must be a SopsSecret (isindir.github.com/v1alpha3)"; return; }
  [[ -n $(yq '.metadata.namespace // ""' "$f") ]] || echo "metadata.namespace is missing"
  [[ $(yq '.sops.encrypted_regex // ""' "$f") == "$yaml_regex" ]] || echo "sops.encrypted_regex must be $yaml_regex"
  [[ $(yq '.spec.secretTemplates | length' "$f") -gt 0 ]] || echo "spec.secretTemplates is empty"
  yq '.spec | keys | .[] | select(. != "secretTemplates" and . != "enforceOwnership" and . != "suspend") | "unexpected field spec." + .' "$f"
  yq '.spec.secretTemplates[] | keys | .[] | select(. != "name" and . != "type" and . != "labels" and . != "annotations" and . != "data" and . != "stringData") | "unexpected field in a secret template: " + .' "$f"
  # Names only: never print a value, it may be plaintext.
  yq '.spec.secretTemplates[] | (.data // {}, .stringData // {}) | to_entries | .[] | select((.value | tag) != "!!str" or (.value | test("^ENC\[AES256_GCM,") | not)) | "value is not encrypted: " + .key' "$f"
}

secret_files() { find "$dir" -mindepth 1 -path "$dir/README.md" -prune -o -path "$access" -prune -o -print | sort; }

policy_path() { echo "${1#"$dir"/}"; }

cmd_check() {
  load
  validate
  local errors=0 f p have want classical problems line
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
    if ! is_secret_path "$p" || ! level_index "${p%%/*}" >/dev/null; then
      echo "secrets: $f: not a secret path (<level>/<name>.sops[.env|.yaml]); plaintext must never live here" >&2
      errors=1; continue
    fi
    have=$(file_recipients "$f")
    if [[ -z $have ]]; then
      echo "secrets: $f: not SOPS-encrypted" >&2; errors=1; continue
    fi
    if [[ $(store "$f") == yaml ]]; then
      problems=$(sops_secret_problems "$f")
      if [[ -n $problems ]]; then
        while read -r line; do echo "secrets: $f: $line" >&2; done <<<"$problems"
        errors=1
      fi
    fi
    classical=$(grep -v '^age1pq1' <<<"$have" || true)
    if [[ -n $classical ]]; then
      echo "secrets: $f: encrypted to a classical (non-post-quantum) recipient: $(while read -r k; do who "$k"; done <<<"$classical" | paste -sd, -)" >&2
      echo "  Re-key with \`just secrets-sync\` and rotate the value: the old one stays readable in git history." >&2
      errors=1
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
  local f=${1:?usage: secrets.sh new deploy/secrets/<level>/<name>.sops[.env|.yaml] < plaintext}
  load
  validate
  local p type problems
  p=$(policy_path "$f")
  [[ $f == "$dir"/* && $(is_secret_path "$p" && echo y) ]] || die "$f: must be $dir/<level>/<name>.sops, .sops.env or .sops.yaml"
  level_index "${p%%/*}" >/dev/null || die "$f: '${p%%/*}' is not one of: ${levels[*]}"
  [[ ! -e $f ]] || die "$f exists: edit it with \`sops edit $f\`"
  diff -q "$config" <(render) >/dev/null || die "$config is out of date: run \`just secrets-sync\` first"
  [[ -t 0 ]] && echo "Paste the value, then Ctrl-D:" >&2
  type=$(store "$f")
  mkdir -p "$(dirname "$f")"
  tmp=$(mktemp)
  trap 'rm -f "$tmp"' EXIT
  sops encrypt --filename-override "$f" --input-type "$type" --output-type "$type" /dev/stdin >"$tmp"
  if [[ $type == yaml ]]; then
    problems=$(sops_secret_problems "$tmp")
    [[ -z $problems ]] || die "$f: $problems"
  fi
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
  # SopsSecrets (.sops.yaml) are for the cluster's operator, and files no machine
  # may read aren't for hosts: neither is written here.
  for f in "$dir/$env"/*.sops "$dir/$env"/*.sops.env; do
    [[ -f $f ]] || continue
    no_machines "$(policy_path "$f")" && continue
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
