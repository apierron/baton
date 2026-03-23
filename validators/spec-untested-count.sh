#!/usr/bin/env bash
# Spec sync: track UNTESTED assertion count.
# Fails if the count increases from the stored baseline.
set -euo pipefail

echo "Checking spec sync: untested assertion count..."

BASELINE_FILE="validators/baselines/untested-count.txt"

# Count current UNTESTED assertions
current=$(grep -rc 'UNTESTED' spec/*.md 2>/dev/null | awk -F: '{s+=$2} END {print s}')

echo "Current UNTESTED count: $current"

if [[ ! -f "$BASELINE_FILE" ]]; then
  echo "No baseline found. Recording current count: $current"
  echo "$current" > "$BASELINE_FILE"
  echo "PASS: Baseline created"
  exit 0
fi

baseline=$(cat "$BASELINE_FILE")
echo "Baseline UNTESTED count: $baseline"

if [[ "$current" -gt "$baseline" ]]; then
  echo "FAIL: UNTESTED count increased from $baseline to $current."
  echo "New assertions must include test references."
  exit 1
fi

if [[ "$current" -lt "$baseline" ]]; then
  echo "UNTESTED count decreased from $baseline to $current. Updating baseline."
  echo "$current" > "$BASELINE_FILE"
fi

echo "PASS: UNTESTED count ($current) <= baseline ($baseline)"
exit 0
