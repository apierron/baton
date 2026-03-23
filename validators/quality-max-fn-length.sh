#!/usr/bin/env bash
# Quality: no function exceeds 100 lines in production code.
# Long functions are a code smell, especially for agent-generated code.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "Checking quality: max function length..."

THRESHOLD=100
VIOLATIONS=0

for file in $(all_rs_files); do
  [[ -f "$file" ]] || continue

  # Use awk to track function boundaries via brace counting
  # Only look at production code (before #[cfg(test)])
  result=$(awk -v threshold="$THRESHOLD" -v fname="$file" '
    /^#\[cfg\(test\)\]/ { exit }

    # Match function declarations
    /^[[:space:]]*(pub )?(async )?fn [a-zA-Z_]/ {
      if (fn_name != "" && fn_lines > threshold) {
        printf "VIOLATION: %s: fn %s is %d lines (threshold: %d)\n", fname, fn_name, fn_lines, threshold
      }
      # Extract function name using gsub
      fn_name = $0
      gsub(/.*fn /, "", fn_name)
      gsub(/[^a-zA-Z0-9_].*/, "", fn_name)
      fn_lines = 0
      depth = 0
      in_fn = 0
    }

    fn_name != "" {
      fn_lines++
      for (i = 1; i <= length($0); i++) {
        c = substr($0, i, 1)
        if (c == "{") { depth++; in_fn = 1 }
        if (c == "}") depth--
        if (in_fn && depth == 0) {
          if (fn_lines > threshold) {
            printf "VIOLATION: %s: fn %s is %d lines (threshold: %d)\n", fname, fn_name, fn_lines, threshold
          }
          fn_name = ""
          fn_lines = 0
          in_fn = 0
          break
        }
      }
    }

    END {
      if (fn_name != "" && fn_lines > threshold) {
        printf "VIOLATION: %s: fn %s is %d lines (threshold: %d)\n", fname, fn_name, fn_lines, threshold
      }
    }
  ' "$file")

  if [[ -n "$result" ]]; then
    echo "$result"
    count=$(echo "$result" | wc -l | tr -d ' ')
    VIOLATIONS=$((VIOLATIONS + count))
  fi
done

if [[ "$VIOLATIONS" -gt 0 ]]; then
  echo "INFO: Found $VIOLATIONS function(s) exceeding $THRESHOLD lines"
  exit 1
fi

echo "PASS: All functions are within $THRESHOLD lines"
exit 0
