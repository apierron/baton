#!/usr/bin/env bash
# Invariant: no global mutable state in production code.
# Per BOUNDARIES.md: "No static mut, no lazy_static with interior mutability,
# no global registries. State flows through function arguments."
set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "Checking invariant: no global mutable state..."

VIOLATIONS=0

for file in $(all_rs_files); do
  [[ -f "$file" ]] || continue

  code=$(prod_code "$file")

  # Check for static mut
  matches=$(echo "$code" | grep -n 'static mut ' || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      echo "VIOLATION: $file: static mut: $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi

  # Check for lazy_static
  matches=$(echo "$code" | grep -n 'lazy_static!' || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      echo "VIOLATION: $file: lazy_static: $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi

  # Check for OnceLock/Lazy with Mutex/RwLock (global mutable state pattern)
  matches=$(echo "$code" | grep -n 'static.*Mutex\|static.*RwLock' || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      echo "VIOLATION: $file: global mutable state: $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi
done

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS global mutable state pattern(s). State must flow through function arguments."
  exit 1
fi

echo "PASS: No global mutable state in production code"
exit 0
