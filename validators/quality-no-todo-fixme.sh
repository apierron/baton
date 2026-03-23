#!/usr/bin/env bash
# Quality: no TODO/FIXME/HACK/XXX markers in production code.
# These are fine during development but shouldn't ship in a release gate.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "Checking quality: no TODO/FIXME/HACK/XXX in production code..."

VIOLATIONS=0

for file in $(all_rs_files); do
  [[ -f "$file" ]] || continue

  code=$(prod_code "$file")

  matches=$(echo "$code" | grep -nE '\b(TODO|FIXME|HACK|XXX)\b' || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      echo "WARNING: $file: $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi
done

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "INFO: Found $VIOLATIONS TODO/FIXME marker(s) in production code"
  # Exit with warn-compatible code (not failure)
  exit 1
fi

echo "PASS: No TODO/FIXME markers in production code"
exit 0
