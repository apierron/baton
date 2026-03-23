#!/usr/bin/env bash
# Invariant: #[cfg(test)] mod tests must appear only at the bottom of each file.
# No production function/struct/impl definitions after the test module.
# Per STYLE.md module layout.
#
# Note: lib.rs has "pub mod test_helpers;" after #[cfg(test)] by design — whitelisted.
# Note: session_common.rs has a test macro that expands #[cfg(test)] — whitelisted.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "Checking invariant: tests only at bottom of files..."

VIOLATIONS=0

for file in $(all_rs_files); do
  [[ -f "$file" ]] || continue

  # Whitelist known exceptions
  base=$(basename "$file")
  [[ "$base" == "lib.rs" ]] && continue
  [[ "$base" == "session_common.rs" ]] && continue

  # Find the line number of #[cfg(test)]
  test_line=$(grep -n '#\[cfg(test)\]' "$file" | head -1 | cut -d: -f1 || true)
  [[ -z "$test_line" ]] && continue

  # Check for multiple #[cfg(test)] blocks
  test_count=$(grep -c '#\[cfg(test)\]' "$file")
  if [[ "$test_count" -gt 1 ]]; then
    echo "VIOLATION: $file: multiple #[cfg(test)] blocks found"
    VIOLATIONS=$((VIOLATIONS + 1))
    continue
  fi

  # Get total lines
  total_lines=$(wc -l < "$file" | tr -d ' ')

  # After the test module, only the closing brace of mod tests and whitespace should remain.
  # The test module is: #[cfg(test)] \n mod tests { ... }
  # We use awk to track brace depth starting from the mod tests { line,
  # then flag any non-whitespace/non-comment after the module closes.
  found_after=$(awk -v start="$test_line" '
    BEGIN { in_test = 0; depth = 0; done = 0 }
    NR < start { next }
    NR == start { in_test = 1; next }
    in_test && !done {
      # Count braces
      for (i = 1; i <= length($0); i++) {
        c = substr($0, i, 1)
        if (c == "{") depth++
        if (c == "}") depth--
        if (depth == 0 && c == "}") { done = 1; break }
      }
      next
    }
    done {
      # After test module closes — flag non-empty, non-comment lines
      if ($0 ~ /^[[:space:]]*$/) next
      if ($0 ~ /^[[:space:]]*\/\//) next
      print NR ": " $0
    }
  ' "$file")

  if [[ -n "$found_after" ]]; then
    while IFS= read -r match; do
      echo "VIOLATION: $file:$match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$found_after"
  fi
done

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS instance(s) of code after #[cfg(test)] module"
  exit 1
fi

echo "PASS: All test modules are at the bottom of their files"
exit 0
