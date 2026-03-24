#!/usr/bin/env bash
# Invariant: no .unwrap() in production code without explicit allowance.
# Per BOUNDARIES.md: "Every unwrap() in non-test code is a potential crash."
set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "Checking invariant: no unwrap() in production code..."

VIOLATIONS=0

for file in $(all_rs_files); do
  [[ -f "$file" ]] || continue

  # Strip #[cfg(test)] block but preserve comments (needed for allow mechanism)
  code=$(sed '/#\[cfg(test)\]/,$d' "$file")

  # Check for .unwrap() — allow if preceded by // baton-allow: unwrap
  while IFS= read -r line_num_and_content; do
    line_num=$(echo "$line_num_and_content" | cut -d: -f1)
    content=$(echo "$line_num_and_content" | cut -d: -f2-)

    # Skip lines that are purely comments
    if echo "$content" | grep -qE '^\s*(//|/\*|\*)'; then
      continue
    fi

    # Check if this line has the allow comment
    if echo "$content" | grep -q 'baton-allow: unwrap'; then
      continue
    fi

    # Check if the preceding line has the allow comment
    if [[ "$line_num" -gt 1 ]]; then
      prev_line=$(echo "$code" | sed -n "$((line_num - 1))p")
      if echo "$prev_line" | grep -q 'baton-allow: unwrap'; then
        continue
      fi
    fi

    echo "VIOLATION: $file:$line_num: $content"
    VIOLATIONS=$((VIOLATIONS + 1))
  done < <(echo "$code" | grep -n '\.unwrap()' || true)
done

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS unwrap() call(s) in production code."
  echo "Use '?' propagation, explicit error handling, or add '// baton-allow: unwrap' with justification."
  exit 1
fi

echo "PASS: No unallowed unwrap() in production code"
exit 0
