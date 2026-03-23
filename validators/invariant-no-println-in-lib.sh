#!/usr/bin/env bash
# Invariant: library modules must not use println!, print!, or dbg!().
# Only main.rs may write to stdout. Baton uses structured JSON output.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "Checking invariant: no println/print/dbg in library code..."

VIOLATIONS=0

for file in $(lib_rs_files); do
  [[ -f "$file" ]] || continue

  code=$(prod_code "$file")

  # Check for println!, print!, dbg! (NOT eprintln — that's for diagnostics)
  matches=$(echo "$code" | grep -nE '\b(println!|dbg!)\(' || true)
  # Also check for bare print! but not eprint! or sprint!
  matches2=$(echo "$code" | grep -nE '([^e])print!\(' || true)
  matches=$(printf '%s\n%s' "$matches" "$matches2" | grep -v '^$' | sort -u || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      echo "VIOLATION: $file: $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi
done

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS println!/print!/dbg!() call(s) in library code."
  echo "Use structured output or eprintln! for diagnostics."
  exit 1
fi

echo "PASS: No println/print/dbg in library code"
exit 0
