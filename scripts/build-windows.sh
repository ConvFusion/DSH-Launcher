#!/usr/bin/env bash
# Cross-compile the Windows (x86_64-pc-windows-msvc) build of DSH Launcher
# on macOS, using Tauri's officially supported cargo-xwin flow.
#
# Usage:
#   scripts/build-windows.sh            # full release build + NSIS installer
#   scripts/build-windows.sh check      # fast cross-target type-check only
#   scripts/build-windows.sh clean      # remove Windows target artifacts
#
# Notes:
#   - Produces the NSIS installer (-Setup.exe) only; .msi requires Windows
#     (WiX) and is excluded via --bundles nsis.
#   - The result is UNSIGNED. Sign on a Windows host / CI (signtool) or via a
#     cloud signing service; macOS codesign cannot sign Windows binaries.
#   - Required tools (one-time setup):
#       cargo install --locked cargo-xwin
#       brew install nsis lld        # + existing: brew install llvm
#
# After a successful build:
#   src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-build}"
TARGET="x86_64-pc-windows-msvc"

# --- Project-local Rust toolchain (same one as the first successful run) ---
export RUSTUP_HOME="$PROJECT_ROOT/.toolchain/rustup"
export CARGO_HOME="$PROJECT_ROOT/.toolchain/cargo"
export PATH="$CARGO_HOME/bin:/opt/homebrew/opt/llvm/bin:/opt/homebrew/bin:$PATH"

# --- Sanity checks -----------------------------------------------------------
missing=0
for tool in cargo-xwin makensis lld-link llvm-rc cargo; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "ERROR: required tool '$tool' not found in PATH." >&2
    missing=1
  fi
done
if [[ $missing -eq 1 ]]; then
  echo "Fix with:  cargo install --locked cargo-xwin && brew install nsis lld" >&2
  exit 1
fi

free_gb=$(df -k "$PROJECT_ROOT" | awk 'NR==2 {print int($4/1024/1024)}')
if [[ $free_gb -lt 10 ]]; then
  echo "WARNING: only ${free_gb}GB free disk; a Windows release build needs ~5-8GB." >&2
fi

cd "$PROJECT_ROOT"

case "$MODE" in
  check)
    # Fast type-check while iterating on code (no release bundle).
    (cd src-tauri && cargo-xwin check --target "$TARGET")
    echo "==> Windows target type-check OK."
    ;;
  build)
    npm run tauri build -- --runner cargo-xwin --target "$TARGET" --bundles nsis
    # Rename the installer so the file name has no spaces (Tauri generates
    # "<ProductName>_<version>_x64-setup.exe" from the product name).
    NSIS_DIR="$PROJECT_ROOT/src-tauri/target/$TARGET/release/bundle/nsis"
    for f in "$NSIS_DIR"/DSH\ Launcher_*-setup.exe; do
      if [[ -f "$f" ]]; then
        base="$(basename "$f")"
        renamed="${base// /_}"
        if [[ "$base" != "$renamed" ]]; then
          mv "$f" "$NSIS_DIR/$renamed"
          echo "==> Renamed installer: $base -> $renamed"
        fi
      fi
    done
    echo "==> Build finished. Installer:"
    echo "    $PROJECT_ROOT/src-tauri/target/$TARGET/release/bundle/nsis/DSH_Launcher_*_x64-setup.exe"
    ;;
  clean)
    rm -rf "src-tauri/target/$TARGET"
    echo "==> Removed src-tauri/target/$TARGET"
    ;;
  *)
    echo "Usage: $0 [check|build|clean]" >&2
    exit 2
    ;;
esac
