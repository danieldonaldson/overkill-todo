#!/usr/bin/env bash
# Create a parallel checkout at ../<repo>-<branch> that can run alongside every
# other one: its own Postgres (Compose project + volume), its own dev, API,
# preview and edge ports, its own backend/.env. Usage: worktree-new.sh <branch> [base]
#
# Everything per-worktree lives in its gitignored .worktree.env, which the
# justfile loads, so plain `just dev` / `just e2e` / `just db-migrate` there
# use that worktree's database and ports.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
branch=${1:?usage: worktree-new.sh <branch> [base-ref]}
base=${2:-HEAD}

main=$(git worktree list --porcelain | sed -n '1s/^worktree //p')
repo=$(basename "$main")
project_base=$(sed -n 's/^name: //p' "$main/compose.yaml")
# Branch -> something valid as a directory and a Compose project name.
slug=$(tr '[:upper:]/' '[:lower:]-' <<<"$branch" | tr -cd 'a-z0-9_-')
[[ -n $slug ]] || { echo "can't derive a name from branch '$branch'" >&2; exit 1; }
dir="$(dirname "$main")/$repo-$slug"
[[ ! -e $dir ]] || { echo "$dir already exists" >&2; exit 1; }

# Slot 0 is the main checkout (default ports); take the lowest slot no live
# worktree holds, so ports get reused after `just wt-rm`.
used=$(git worktree list --porcelain | sed -n 's/^worktree //p' | while IFS= read -r p; do
  [[ -f $p/.worktree.env ]] && sed -n 's/^WT_SLOT=//p' "$p/.worktree.env"
done || true)
slot=1
while grep -qx "$slot" <<<"$used"; do slot=$((slot + 1)); done
((slot < 50)) || { echo "no free worktree slot" >&2; exit 1; }

if git show-ref --verify --quiet "refs/heads/$branch"; then
  git worktree add "$dir" "$branch"
else
  git worktree add -b "$branch" "$dir" "$base"
fi

cat >"$dir/.worktree.env" <<ENV
# Written by scripts/worktree-new.sh; loaded by the justfile. Main checkout = slot 0.
WT_SLOT=$slot
COMPOSE_PROJECT_NAME=$project_base-$slug
DB_PORT=$((5432 + slot))
WEB_PORT=$((4321 + 10 * slot))
API_PORT=$((4322 + 10 * slot))
PREVIEW_PORT=$((4323 + 10 * slot))
EDGE_PORT=$((8080 + slot))
ENV

cd "$dir"
mise trust --quiet 2>/dev/null || true
# Exported rather than relying on the checkout's justfile loading .worktree.env,
# so this also works when <base> predates that.
set -a
. ./.worktree.env
set +a
env PNPM_CONFIG_MINIMUM_RELEASE_AGE=1440 mise exec -- pnpm install --frozen-lockfile
"$here/gen-dev-env.sh" "$dir"
just db-up db-migrate

echo
echo "Ready: $dir (branch $branch, slot $slot)"
sed -n 's/^\(.*_PORT\)=/  \1=/p' .worktree.env
echo "  cd $dir && just dev    # then open http://localhost:$((4321 + 10 * slot))"
