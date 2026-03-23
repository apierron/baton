#!/usr/bin/env bash
# Security: artifact/file content must never appear in error messages.
# Per BOUNDARIES.md: "Never log or include input file content in error messages —
# input files may contain secrets."
set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "Checking security: no file content in error messages..."

VIOLATIONS=0

for file in $(all_rs_files); do
  [[ -f "$file" ]] || continue

  code=$(prod_code "$file")

  # Check #[error("...")] format strings for content interpolation
  matches=$(echo "$code" | grep -nE '#\[error\(' | grep -iE '\{.*(content|artifact_content|file_content|raw_content).*\}' || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      echo "VIOLATION: $file: error format leaks content: $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi

  # Check eprintln!/format! for content interpolation in error paths
  matches=$(echo "$code" | grep -nE '(eprintln!|panic!)' | grep -iE '(content|artifact_content|file_content|raw_content)' || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      echo "VIOLATION: $file: error output leaks content: $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi
done

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS error message(s) that may leak file content."
  exit 1
fi

echo "PASS: No file content in error messages"
exit 0
