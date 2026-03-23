#!/usr/bin/env bash
# Spec sync: validate SPEC assertion ID format.
# Format: SPEC-XX-YY-NNN: kebab-case-description
set -euo pipefail

echo "Checking spec format: assertion ID validity..."

VIOLATIONS=0
SEEN_IDS=""

for spec_file in spec/*.md; do
  [[ -f "$spec_file" ]] || continue

  # Extract all SPEC- lines
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue

    # Validate format: SPEC-XX-YY-NNN or SPEC-XX-YY-ZZ-NNN: kebab-description
    if ! echo "$line" | grep -qE '^SPEC-[A-Z]{2,3}(-[A-Z]{2,3}){1,3}-[0-9]{3}: [a-z0-9][-a-z0-9]*$'; then
      echo "VIOLATION: $spec_file: malformed assertion ID: $line"
      VIOLATIONS=$((VIOLATIONS + 1))
      continue
    fi

    # Extract the ID portion
    id=$(echo "$line" | cut -d: -f1)

    # Check for duplicate IDs
    if echo "$SEEN_IDS" | grep -qF "$id"; then
      echo "VIOLATION: $spec_file: duplicate assertion ID: $id"
      VIOLATIONS=$((VIOLATIONS + 1))
    fi
    SEEN_IDS="$SEEN_IDS $id"
  done < <(grep '^SPEC-' "$spec_file" || true)
done

total=$(echo "$SEEN_IDS" | wc -w | tr -d ' ')
echo "Checked $total assertion IDs"

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS spec format violation(s)"
  exit 1
fi

echo "PASS: All spec assertion IDs are well-formed"
exit 0
