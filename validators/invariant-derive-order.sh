#!/usr/bin/env bash
# Invariant: derive attributes follow canonical order.
# Per STYLE.md: Debug, Clone, Serialize, Deserialize (skip unused, but preserve relative order).
set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "Checking invariant: derive attribute order..."

VIOLATIONS=0

# Canonical order (only these four are checked; others can appear anywhere)
ORDERED=(Debug Clone Serialize Deserialize)

for file in $(all_rs_files); do
  [[ -f "$file" ]] || continue

  code=$(prod_code "$file")

  # Find all #[derive(...)] lines
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    line_num=$(echo "$line" | cut -d: -f1)
    derives=$(echo "$line" | sed 's/.*#\[derive(\(.*\))\].*/\1/' | tr ',' '\n' | sed 's/^ *//;s/ *$//')

    # Extract positions of our canonical derives
    last_pos=-1
    for canonical in "${ORDERED[@]}"; do
      pos=0
      found=false
      while IFS= read -r d; do
        if [[ "$d" == "$canonical" ]]; then
          found=true
          break
        fi
        pos=$((pos + 1))
      done <<< "$derives"

      if $found; then
        if [[ $pos -lt $last_pos ]]; then
          echo "VIOLATION: $file:$line_num: '$canonical' appears before a preceding canonical derive. Expected order: Debug, Clone, Serialize, Deserialize"
          VIOLATIONS=$((VIOLATIONS + 1))
          break
        fi
        last_pos=$pos
      fi
    done
  done < <(echo "$code" | grep -n '#\[derive(' || true)
done

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS derive ordering violation(s)"
  exit 1
fi

echo "PASS: All derive attributes follow canonical order"
exit 0
