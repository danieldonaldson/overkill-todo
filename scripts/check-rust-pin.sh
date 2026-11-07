#!/usr/bin/env bash
# Fail if the Rust version mise installs (.mise.toml) differs from the one CI and
# the Docker build use (backend/rust-toolchain.toml).
set -euo pipefail
cd "$(dirname "$0")/.."
mise_v=$(sed -n 's/^rust = .*version = "\([^"]*\)".*/\1/p' .mise.toml)
file_v=$(sed -n 's/^channel = "\([^"]*\)".*/\1/p' backend/rust-toolchain.toml)
if [ -z "$mise_v" ] || [ -z "$file_v" ]; then
  echo "could not read a Rust version from .mise.toml ('$mise_v') or backend/rust-toolchain.toml ('$file_v')" >&2
  exit 1
fi
if [ "$mise_v" != "$file_v" ]; then
  echo "Rust pins drifted: .mise.toml has $mise_v, backend/rust-toolchain.toml has $file_v" >&2
  exit 1
fi
echo "Rust pinned to $mise_v in both places"
