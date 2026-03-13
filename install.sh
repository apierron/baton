#!/usr/bin/env bash
set -euo pipefail

# Baton installer
# Usage: curl -fsSL https://raw.githubusercontent.com/apierron/baton/master/install.sh | bash

REPO="apierron/baton"
BINARY="baton"

# Detect OS and architecture
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    os="unknown-linux-gnu"
    EXT="tar.gz"
    ;;
  Darwin)
    os="apple-darwin"
    EXT="tar.gz"
    ;;
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    os="pc-windows-msvc"
    EXT="zip"
    BINARY="baton.exe"
    ;;
  *)
    echo "Error: unsupported OS: $OS"
    exit 1
    ;;
esac

case "$ARCH" in
  x86_64|x86-64|AMD64)  arch="x86_64" ;;
  aarch64|arm64|ARM64)   arch="aarch64" ;;
  *)       echo "Error: unsupported architecture: $ARCH"; exit 1 ;;
esac

# Default install dir: use Program Files on Windows, ~/.local/bin elsewhere
if [ "$os" = "pc-windows-msvc" ]; then
  INSTALL_DIR="${BATON_INSTALL_DIR:-${LOCALAPPDATA:-$HOME/AppData/Local}/baton}"
else
  INSTALL_DIR="${BATON_INSTALL_DIR:-$HOME/.local/bin}"
fi

TARGET="${arch}-${os}"

# Get latest release tag
LATEST=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "$LATEST" ]; then
  echo "Error: could not determine latest release"
  exit 1
fi

echo "Installing baton ${LATEST} for ${TARGET}..."

# Download and extract
URL="https://github.com/${REPO}/releases/download/${LATEST}/baton-${LATEST}-${TARGET}.${EXT}"
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

if [ "$EXT" = "zip" ]; then
  curl -fsSL "$URL" -o "${TMPDIR}/baton.zip"
  unzip -q "${TMPDIR}/baton.zip" -d "$TMPDIR"
else
  curl -fsSL "$URL" -o "${TMPDIR}/baton.tar.gz"
  tar -xzf "${TMPDIR}/baton.tar.gz" -C "$TMPDIR"
fi

# Install
mkdir -p "$INSTALL_DIR"
mv "${TMPDIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
chmod +x "${INSTALL_DIR}/${BINARY}" 2>/dev/null || true

echo "Installed baton to ${INSTALL_DIR}/${BINARY}"

# Check if INSTALL_DIR is in PATH
case "$PATH" in
  *"$INSTALL_DIR"*) ;;
  *)
    echo ""
    if [ "$os" = "pc-windows-msvc" ]; then
      echo "Add ${INSTALL_DIR} to your PATH via System Settings > Environment Variables"
    else
      echo "Add this to your shell profile to put baton in your PATH:"
      echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    fi
    ;;
esac
