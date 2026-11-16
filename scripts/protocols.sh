#!/usr/bin/env bash
# Tamarin protocol model (OT-010 design D9), run by `just protocols` and CI:
#
#   1. Every lemma in models/auth_protocols.spthy is proven for the code as it
#      stands, with no wellformedness warnings.
#   2. Each broken variant (a check the code makes, removed with a -D flag)
#      must still be attacked, so a lemma can't silently become vacuous.
#
# Tamarin exits 0 whatever it finds, so this reads its summary. Needs
# tamarin-prover and maude on PATH (pinned in .mise.toml). Run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v tamarin-prover >/dev/null && command -v maude >/dev/null ||
  { echo "protocols: Tamarin or Maude isn't installed: run 'mise install'" >&2; exit 1; }

model=models/auth_protocols.spthy
limit=${PROTOCOLS_TIMEOUT:-600} # seconds per run

prove() { # extra tamarin args...
  timeout "$limit" tamarin-prover "$model" "$@" 2>&1 ||
    { echo "protocols: tamarin-prover failed or ran past ${limit}s" >&2; return 1; }
}

echo "protocols: every lemma holds"
out=$(prove --prove) || { echo "$out" >&2; exit 1; }
summary=$(sed -n '/summary of summaries/,$p' <<<"$out")
echo "$summary"
if grep -q 'WARNING' <<<"$out"; then
  echo "$out" | grep -A20 'WARNING' >&2
  echo "protocols: the model has wellformedness warnings" >&2
  exit 1
fi
if ! grep -q ': verified' <<<"$summary" || grep -Eq 'falsified|analysis incomplete' <<<"$summary"; then
  echo "protocols: a lemma isn't proven" >&2
  exit 1
fi

defines() { # FLAG[+FLAG...] -> -D=FLAG ...
  local flag
  for flag in ${1//+/ }; do printf -- '-D=%s\n' "$flag"; done
}

# The [reuse] helper lemmas are assumed by every lemma after them, so a
# variant run proves them too and they must still hold there; otherwise a
# variant's result could rest on a helper its flags made false.
helpers=(server_key_secret fingerprint_known_only_if_stolen)

variant() { # flags lemma -> the lemma's verdict ("verified" or "falsified")
  local out helper ds args=()
  mapfile -t ds < <(defines "$1")
  for helper in "${helpers[@]}"; do args+=(--prove="$helper"); done
  out=$(prove "${args[@]}" --prove="$2" "${ds[@]}") || { echo "$out" >&2; exit 1; }
  for helper in "${helpers[@]}"; do
    if ! grep -Eq "^ *$helper \(all-traces\): verified" <<<"$out"; then
      sed -n '/summary of summaries/,$p' <<<"$out" >&2
      echo "protocols: helper $helper doesn't hold with $1" >&2
      exit 1
    fi
  done
  grep -Eo "^ *$2 \(all-traces\): (verified|falsified)" <<<"$out" | grep -Eo '(verified|falsified)$' || {
    sed -n '/summary of summaries/,$p' <<<"$out" >&2
    echo "protocols: no verdict for $2 with $1" >&2
    exit 1
  }
}

expect() { # verdict flags lemma
  local got
  got=$(variant "$2" "$3")
  if [[ $got != "$1" ]]; then
    echo "protocols: with $2, expected $3 to be $1, but it was $got" >&2
    exit 1
  fi
}

attacked() { # flags lemma: with those checks removed, the lemma must have an attack
  echo "protocols: with $1, $2 is attacked, as it must be"
  expect falsified "$1" "$2"
}

holds() { # flags lemma: under a weaker assumption, the lemma must still be proven
  echo "protocols: with $1, $2 still holds"
  expect verified "$1" "$2"
}

# Each check the code makes, removed: the lemma that relies on it must break.
attacked INSECURE_CHANNEL browser_tokens_stay_secret
attacked INSECURE_CHANNEL replayed_sign_in_gains_nothing
attacked NO_REVOKED_CHECK refresh_replay_gains_nothing
attacked NO_REVOKE_ALL stolen_reuse_ends_every_session
attacked NO_FINGERPRINT refresh_needs_the_fingerprint
attacked NO_STATE_CHECK google_sign_in_completes_where_it_started
attacked NO_STATE_CHECK no_login_csrf
attacked NO_CSRF_CHECK cross_site_pages_cannot_change_state
attacked ACCEPT_SIMPLE_BODIES exempt_routes_cannot_be_driven_cross_site
# The CSRF token's user binding is what stands when the attacker can write
# cookies: without the `__Host-` prefix it still holds, and without the
# binding as well it breaks.
holds NO_HOST_PREFIX cross_site_changes_need_a_token_signed_for_the_member
attacked NO_HOST_PREFIX+NO_CSRF_BINDING cross_site_changes_need_a_token_signed_for_the_member
# The code before the `__Host-` prefix: an attacker who can write the app's
# cookies gets login CSRF, through the oauth_state cookie or the session cookies.
attacked NO_HOST_PREFIX google_sign_in_completes_where_it_started
attacked NO_HOST_PREFIX no_login_csrf
