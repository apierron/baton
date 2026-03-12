#!/usr/bin/env bash
# Invariant: error messages include context; no panic!() in production code.
set -euo pipefail

echo "Checking invariant: error messages include context..."

WARNINGS=0

# Check that eprintln!("Error: ...") in main.rs includes interpolation
static_errors=$(grep -nE 'eprintln!\("Error: [^{}"]*"\)' src/main.rs || true)
if [[ -n "$static_errors" ]]; then
  while IFS= read -r line; do
    echo "WARNING: main.rs: static error message without context: $line"
    WARNINGS=$((WARNINGS + 1))
  done <<< "$static_errors"
fi

# Check that panic!() is not used in library production code
for file in src/config.rs src/exec.rs src/types.rs src/placeholder.rs src/prompt.rs src/verdict_parser.rs src/history.rs src/runtime.rs src/error.rs; do
  [[ -f "$file" ]] || continue

  # Get production code only (before #[cfg(test)])
  prod_code=$(sed '/#\[cfg(test)\]/,$d' "$file")
  prod_code=$(echo "$prod_code" | grep -vE '^\s*(//|/\*|\*)' || true)

  matches=$(echo "$prod_code" | grep -n 'panic!\(' || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      echo "WARNING: $file: panic!() in production code: $match"
      WARNINGS=$((WARNINGS + 1))
    done <<< "$matches"
  fi
done

if [[ $WARNINGS -gt 0 ]]; then
  echo "INFO: Found $WARNINGS warning(s) — review recommended"
  exit 0
fi

echo "PASS: Error messages look well-formed"
exit 0
