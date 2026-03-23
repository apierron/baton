#!/usr/bin/env bash
# Regression guard: spec assertion count must not decrease.
# Prevents accidental spec assertion deletion.
set -euo pipefail

echo "Checking regression: spec assertion count..."

BASELINE_FILE="validators/baselines/spec-assertion-count.txt"

# Count SPEC- assertions across all spec files
current=$(grep -rc '^SPEC-' spec/*.md 2>/dev/null | awk -F: '{s+=$2} END {print s}')

echo "Current spec assertion count: $current"

if [[ ! -f "$BASELINE_FILE" ]]; then
  echo "No baseline found. Recording current count: $current"
  echo "$current" > "$BASELINE_FILE"
  echo "PASS: Baseline created"
  exit 0
fi

baseline=$(cat "$BASELINE_FILE")
echo "Baseline spec assertion count: $baseline"

if [[ "$current" -lt "$baseline" ]]; then
  echo "FAIL: Spec assertion count decreased from $baseline to $current."
  echo "Spec assertions should not be removed."
  exit 1
fi

if [[ "$current" -gt "$baseline" ]]; then
  echo "Spec assertion count increased from $baseline to $current. Updating baseline."
  echo "$current" > "$BASELINE_FILE"
fi

echo "PASS: Spec assertion count ($current) >= baseline ($baseline)"
exit 0
