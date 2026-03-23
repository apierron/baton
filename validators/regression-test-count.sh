#!/usr/bin/env bash
# Regression guard: test count must not decrease.
# Prevents accidental test deletion.
set -euo pipefail

echo "Checking regression: test count..."

BASELINE_FILE="validators/baselines/test-count.txt"

# Count #[test] annotations in src/ and tests/
current=$(grep -rc '#\[test\]' src/ tests/ 2>/dev/null | awk -F: '{s+=$2} END {print s}')

echo "Current test count: $current"

if [[ ! -f "$BASELINE_FILE" ]]; then
  echo "No baseline found. Recording current count: $current"
  echo "$current" > "$BASELINE_FILE"
  echo "PASS: Baseline created"
  exit 0
fi

baseline=$(cat "$BASELINE_FILE")
echo "Baseline test count: $baseline"

if [[ "$current" -lt "$baseline" ]]; then
  echo "FAIL: Test count decreased from $baseline to $current."
  echo "Tests should not be deleted without justification."
  exit 1
fi

if [[ "$current" -gt "$baseline" ]]; then
  echo "Test count increased from $baseline to $current. Updating baseline."
  echo "$current" > "$BASELINE_FILE"
fi

echo "PASS: Test count ($current) >= baseline ($baseline)"
exit 0
