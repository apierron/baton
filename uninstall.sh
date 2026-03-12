#!/usr/bin/env bash
set -euo pipefail

# Baton uninstaller

INSTALL_DIR="${BATON_INSTALL_DIR:-$HOME/.local/bin}"
BINARY="baton"

TARGET="${INSTALL_DIR}/${BINARY}"

if [ -f "$TARGET" ]; then
  rm "$TARGET"
  echo "Removed ${TARGET}"
else
  # Also check cargo install location
  CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin/${BINARY}"
  if [ -f "$CARGO_BIN" ]; then
    cargo uninstall baton 2>/dev/null && echo "Uninstalled baton via cargo" || rm "$CARGO_BIN"
  else
    echo "baton not found at ${TARGET} or ${CARGO_BIN}"
    exit 1
  fi
fi

echo "baton has been uninstalled."
