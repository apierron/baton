#!/usr/bin/env bash
# Invariant: production code must use BTreeMap, never HashMap.
# BTreeMap provides deterministic ordering for hashing and output.
set -euo pipefail

echo "Checking invariant: no HashMap in production code..."

# Strategy: grep for HashMap in src/, then exclude test modules.
# Test modules start at "#[cfg(test)]" and go to EOF, so we strip those sections.

VIOLATIONS=0

for file in src/*.rs; do
  [[ -f "$file" ]] || continue

  # Extract only the production portion (before #[cfg(test)])
  prod_code=$(sed '/#\[cfg(test)\]/,$d' "$file")

  # Remove comment lines
  prod_code=$(echo "$prod_code" | grep -vE '^\s*(//|/\*|\*)' || true)

  # Search for HashMap
  matches=$(echo "$prod_code" | grep -n 'HashMap' || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      echo "VIOLATION: $file: $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi
done

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS HashMap usage(s) in production code. Use BTreeMap instead."
  exit 1
fi

echo "PASS: No HashMap found in production code"
exit 0
