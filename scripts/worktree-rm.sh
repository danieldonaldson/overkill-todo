#!/usr/bin/env bash
# Remove a worktree made by worktree-new.sh and its database container + volume.
# Refuses a dirty worktree unless FORCE=1. Keeps the branch.
# Usage: worktree-rm.sh <branch>
set -euo pipefail

branch=${1:?usage: worktree-rm.sh <branch>}
main=$(git worktree list --porcelain | sed -n '1s/^worktree //p')
slug=$(tr '[:upper:]/' '[:lower:]-' <<<"$branch" | tr -cd 'a-z0-9_-')
dir="$(dirname "$main")/$(basename "$main")-$slug"

[[ -f $dir/.worktree.env ]] || { echo "no worktree made by wt-new at $dir" >&2; exit 1; }
if [[ ${FORCE:-0} != 1 && -n $(git -C "$dir" status --porcelain) ]]; then
  echo "$dir has uncommitted changes (FORCE=1 to discard them):" >&2
  git -C "$dir" status --short >&2
  exit 1
fi

project=$(sed -n 's/^COMPOSE_PROJECT_NAME=//p' "$dir/.worktree.env")
[[ -n $project ]] || { echo "no COMPOSE_PROJECT_NAME in $dir/.worktree.env" >&2; exit 1; }
# -f: the compose file lives in the worktree being removed.
docker compose -f "$dir/compose.yaml" -p "$project" --profile full down -v --remove-orphans

if [[ ${FORCE:-0} == 1 ]]; then git worktree remove --force "$dir"; else git worktree remove "$dir"; fi
echo "Removed $dir and the $project database. Branch '$branch' kept (git branch -d $branch)."
