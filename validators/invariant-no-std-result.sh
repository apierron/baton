#!/usr/bin/env bash
# Invariant: use crate::error::Result<T>, never std::result::Result<_, BatonError>.
# Per STYLE.md error handling conventions.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "Checking invariant: no direct std::result::Result with BatonError..."

VIOLATIONS=0

for file in $(all_rs_files); do
  [[ -f "$file" ]] || continue

  # Skip error.rs where the alias is defined
  [[ "$(basename "$file")" == "error.rs" ]] && continue

  code=$(prod_code "$file")

  # Look for std::result::Result paired with BatonError on the same line
  matches=$(echo "$code" | grep -n 'std::result::Result' | grep -i 'BatonError' || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      # Whitelist: serde Deserialize impls and FromStr impls use std::result::Result<Self, D::Error>
      if echo "$match" | grep -qE '(D::Error|Self::Err|de::Error)'; then
        continue
      fi
      echo "VIOLATION: $file: $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi

  # Also check for bare Result<_, BatonError> without the crate alias
  # (someone might import std::result::Result and use it directly)
  matches=$(echo "$code" | grep -n 'Result<.*BatonError>' | grep -v 'crate::error::Result' || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      # Skip the error.rs type alias definition itself and trait impls
      if echo "$match" | grep -qE '(type Result|D::Error|Self::Err|de::Error)'; then
        continue
      fi
      # Skip lines that are just using Result<T> (the crate alias)
      if echo "$match" | grep -qE 'Result<[^,>]+>'; then
        continue
      fi
      echo "VIOLATION: $file: $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi
done

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS direct std::result::Result<_, BatonError> usage(s)."
  echo "Use crate::error::Result<T> instead."
  exit 1
fi

echo "PASS: All code uses crate::error::Result<T>"
exit 0
