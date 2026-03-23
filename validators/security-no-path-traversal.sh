#!/usr/bin/env bash
# Security: file-loading code should not blindly follow ../ path traversal.
# Checks that user-supplied file paths are validated before loading.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "Checking security: path traversal protections..."

WARNINGS=0

# Check if prompt.rs, exec.rs, or types.rs join user paths without canonicalization
for file in src/prompt.rs src/exec.rs src/types.rs; do
  [[ -f "$file" ]] || continue

  code=$(prod_code "$file")

  # Look for path joining with user-supplied values without canonicalize/strip_prefix
  # This is a heuristic — flag .join() calls and check if canonicalize exists nearby
  join_count=$(echo "$code" | grep -c '\.join(' || true)
  canon_count=$(echo "$code" | grep -c 'canonicalize\|strip_prefix\|starts_with' || true)

  if [[ "$join_count" -gt 0 && "$canon_count" -eq 0 ]]; then
    echo "WARNING: $file: has $join_count path .join() call(s) but no canonicalize/strip_prefix checks"
    WARNINGS=$((WARNINGS + 1))
  fi
done

if [[ $WARNINGS -gt 0 ]]; then
  echo "INFO: Found $WARNINGS file(s) that may need path traversal protection. Review recommended."
  # Exit 0 — this is advisory, not blocking
  exit 0
fi

echo "PASS: Path traversal checks look reasonable"
exit 0
