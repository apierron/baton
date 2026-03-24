#!/usr/bin/env bash
# Invariant: version strings must not be hardcoded in Rust source.
# Per CONVENTIONS.md: "The version is defined once in Cargo.toml.
# All runtime references use env!("CARGO_PKG_VERSION")."
set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "Checking invariant: no hardcoded version strings in source..."

# Extract the current version from Cargo.toml
VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
if [[ -z "$VERSION" ]]; then
  echo "ERROR: Could not extract version from Cargo.toml"
  exit 2
fi

echo "Current version: $VERSION"

VIOLATIONS=0

for file in $(all_rs_files); do
  [[ -f "$file" ]] || continue

  code=$(prod_code "$file")

  # Search for the exact version string in quotes (e.g., "0.6.2")
  matches=$(echo "$code" | grep -n "\"$VERSION\"" || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      # Whitelist: env!("CARGO_PKG_VERSION") comparisons are fine
      if echo "$match" | grep -q 'env!'; then
        continue
      fi
      # Whitelist: config.rs version format checks ("0.4", "0.5", "0.6", "0.7") are config schema versions
      if [[ "$(basename "$file")" == "config.rs" ]] && echo "$match" | grep -qE '"0\.[0-9]+"'; then
        continue
      fi
      echo "VIOLATION: $file: hardcoded version string: $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi
done

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS hardcoded version string(s). Use env!(\"CARGO_PKG_VERSION\") instead."
  exit 1
fi

echo "PASS: No hardcoded version strings found"
exit 0
