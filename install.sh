#!/usr/bin/env bash
set -euo pipefail

# Baton installer
# Usage: curl -fsSL https://raw.githubusercontent.com/apierron/baton/master/install.sh | bash

REPO="apierron/baton"
INSTALL_DIR="${BATON_INSTALL_DIR:-$HOME/.local/bin}"
BINARY="baton"

# Detect OS and architecture
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)   os="unknown-linux-gnu" ;;
  Darwin)  os="apple-darwin" ;;
  *)       echo "Error: unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
  x86_64)  arch="x86_64" ;;
  aarch64|arm64) arch="aarch64" ;;
  *)       echo "Error: unsupported architecture: $ARCH"; exit 1 ;;
esac

TARGET="${arch}-${os}"

# Get latest release tag
LATEST=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "$LATEST" ]; then
  echo "Error: could not determine latest release"
  exit 1
fi

echo "Installing baton ${LATEST} for ${TARGET}..."

# Download
URL="https://github.com/${REPO}/releases/download/${LATEST}/baton-${LATEST}-${TARGET}.tar.gz"
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

curl -fsSL "$URL" -o "${TMPDIR}/baton.tar.gz"
tar -xzf "${TMPDIR}/baton.tar.gz" -C "$TMPDIR"

# Install
mkdir -p "$INSTALL_DIR"
mv "${TMPDIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
chmod +x "${INSTALL_DIR}/${BINARY}"

echo "Installed baton to ${INSTALL_DIR}/${BINARY}"

# Check if INSTALL_DIR is in PATH
if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
  echo ""
  echo "Add this to your shell profile to put baton in your PATH:"
  echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
fi
