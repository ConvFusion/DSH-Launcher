#!/usr/bin/env bash
# Ensure the project-local Rust toolchain is used for tauri commands.
# The project builds with a sandbox-local cargo/rustup (.toolchain/) so the
# fingerprint cache stays consistent — otherwise cargo falls back to the
# user's ~/.cargo and recompiles everything on every run.
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ -d "$PROJECT_ROOT/.toolchain/cargo" && -d "$PROJECT_ROOT/.toolchain/rustup" ]]; then
  export CARGO_HOME="$PROJECT_ROOT/.toolchain/cargo"
  export RUSTUP_HOME="$PROJECT_ROOT/.toolchain/rustup"
  export PATH="$PROJECT_ROOT/.toolchain/cargo/bin:$PATH"
fi

exec "$@"
