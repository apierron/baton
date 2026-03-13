#!/usr/bin/env bash
set -euo pipefail

# Baton uninstaller
# Discovers all baton installations, lists them, and lets you choose what to remove.
# Usage: curl -fsSL https://raw.githubusercontent.com/apierron/baton/master/uninstall.sh | bash

BINARY="baton"

# Detect platform
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    SCRIPT_DIR="${BATON_INSTALL_DIR:-${LOCALAPPDATA:-$HOME/AppData/Local}/baton}"
    BINARY="baton.exe"
    ;;
  *)
    SCRIPT_DIR="${BATON_INSTALL_DIR:-$HOME/.local/bin}"
    ;;
esac

# --- Discovery ---
# Each entry is "path|method" where method is how to remove it
INSTALLATIONS=()

# 1. Script install location
if [ -f "${SCRIPT_DIR}/${BINARY}" ]; then
  INSTALLATIONS+=("${SCRIPT_DIR}/${BINARY}|file")
fi

# 2. Cargo install location
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin/baton"
if [ -f "$CARGO_BIN" ]; then
  INSTALLATIONS+=("${CARGO_BIN}|cargo")
fi

# 3. Homebrew
if command -v brew &>/dev/null && brew list baton &>/dev/null 2>&1; then
  BREW_BIN="$(brew --prefix)/bin/baton"
  INSTALLATIONS+=("${BREW_BIN}|brew")
fi

# 4. Common system paths (might catch manual installs)
for dir in /usr/local/bin /opt/homebrew/bin; do
  candidate="${dir}/baton"
  [ -f "$candidate" ] || continue
  # Skip if already found via homebrew
  already=0
  for entry in "${INSTALLATIONS[@]+"${INSTALLATIONS[@]}"}"; do
    entry_path="${entry%%|*}"
    # Resolve symlinks for comparison
    real_candidate="$(readlink -f "$candidate" 2>/dev/null || echo "$candidate")"
    real_entry="$(readlink -f "$entry_path" 2>/dev/null || echo "$entry_path")"
    if [ "$real_candidate" = "$real_entry" ]; then
      already=1
      break
    fi
  done
  [ "$already" -eq 0 ] && INSTALLATIONS+=("${candidate}|file")
done

# --- Report ---
if [ ${#INSTALLATIONS[@]} -eq 0 ]; then
  echo "No baton installations found."
  exit 1
fi

echo "Found the following baton installation(s):"
echo ""
for i in "${!INSTALLATIONS[@]}"; do
  entry="${INSTALLATIONS[$i]}"
  path="${entry%%|*}"
  method="${entry##*|}"
  case "$method" in
    cargo) label="(installed via cargo)" ;;
    brew)  label="(installed via homebrew)" ;;
    file)  label="" ;;
    *)     label="" ;;
  esac
  printf "  [%d] %s %s\n" "$((i + 1))" "$path" "$label"
done

echo ""

if [ ${#INSTALLATIONS[@]} -eq 1 ]; then
  printf "Remove this installation? [y/N] "
  read -r answer
  case "$answer" in
    y|Y|yes|YES) SELECTED=(0) ;;
    *) echo "Aborted."; exit 0 ;;
  esac
else
  echo "Enter the numbers to remove (comma-separated), 'all' to remove everything, or 'q' to quit:"
  printf "> "
  read -r answer
  case "$answer" in
    q|Q|quit|"") echo "Aborted."; exit 0 ;;
    all|ALL|a|A)
      SELECTED=()
      for i in "${!INSTALLATIONS[@]}"; do
        SELECTED+=("$i")
      done
      ;;
    *)
      IFS=',' read -ra SELECTED_RAW <<< "$answer"
      SELECTED=()
      for s in "${SELECTED_RAW[@]}"; do
        s="$(echo "$s" | tr -d ' ')"
        idx=$((s - 1))
        if [ "$idx" -lt 0 ] || [ "$idx" -ge ${#INSTALLATIONS[@]} ]; then
          echo "Invalid selection: $s"
          exit 1
        fi
        SELECTED+=("$idx")
      done
      ;;
  esac
fi

# --- Removal ---
FAILED=0

for idx in "${SELECTED[@]}"; do
  entry="${INSTALLATIONS[$idx]}"
  path="${entry%%|*}"
  method="${entry##*|}"

  case "$method" in
    cargo)
      if cargo uninstall baton 2>/dev/null; then
        echo "Uninstalled baton via cargo."
      else
        # Fallback: delete the binary directly
        rm -f "$path" && echo "Removed ${path}" || { echo "Error removing ${path}"; FAILED=1; }
      fi
      ;;
    brew)
      if brew uninstall baton 2>/dev/null; then
        echo "Uninstalled baton via Homebrew."
      else
        echo "Error: brew uninstall failed. Try running 'brew uninstall baton' manually."
        FAILED=1
      fi
      ;;
    file)
      rm -f "$path" && echo "Removed ${path}" || { echo "Error removing ${path}"; FAILED=1; }
      ;;
  esac
done

# Check for leftovers
REMAINING="$(command -v baton 2>/dev/null || true)"
if [ -n "$REMAINING" ]; then
  echo ""
  echo "Warning: baton is still found at: ${REMAINING}"
  echo "You may need to remove it manually."
fi

if [ "$FAILED" -ne 0 ]; then
  echo "Some installations could not be removed."
  exit 1
fi

echo ""
echo "baton has been uninstalled."
