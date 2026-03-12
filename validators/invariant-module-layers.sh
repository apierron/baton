#!/usr/bin/env bash
# Invariant: module dependency layers must flow downward only.
#
# Layer rules (from AGENTS.md):
#   main.rs → exec, config, history, runtime, types
#   exec → config, types, placeholder, runtime, error
#   config → types, placeholder, error
#   history, placeholder → types, error
#   runtime → types, error
#   prompt, verdict_parser → types or error only
#   error → (leaf, no internal imports)
set -euo pipefail

echo "Checking invariant: module dependency layers..."

VIOLATIONS=0

check_no_import() {
  local module="$1"
  local forbidden="$2"
  local file="src/${module}.rs"

  [[ -f "$file" ]] || return 0

  # Get production code only (before #[cfg(test)])
  local prod_code
  prod_code=$(sed '/#\[cfg(test)\]/,$d' "$file")

  # Remove comments
  prod_code=$(echo "$prod_code" | grep -vE '^\s*(//|/\*|\*)' || true)

  # Check for forbidden imports
  local matches
  matches=$(echo "$prod_code" | grep -n "use crate::${forbidden}" || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      echo "VIOLATION: $file imports '$forbidden' (layer violation): $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi
}

# error.rs is a leaf — must not import any crate modules
for mod in config exec history placeholder prompt runtime types verdict_parser; do
  check_no_import "error" "$mod"
done

# prompt and verdict_parser → only types or error
for forbidden in config exec history placeholder runtime; do
  check_no_import "prompt" "$forbidden"
  check_no_import "verdict_parser" "$forbidden"
done

# placeholder → only types or error
for forbidden in config exec history runtime prompt verdict_parser; do
  check_no_import "placeholder" "$forbidden"
done

# history → only types or error
for forbidden in config exec placeholder runtime prompt verdict_parser; do
  check_no_import "history" "$forbidden"
done

# runtime → only types or error
for forbidden in config exec history placeholder prompt verdict_parser; do
  check_no_import "runtime" "$forbidden"
done

# config → only types, placeholder, error
for forbidden in exec history runtime prompt verdict_parser; do
  check_no_import "config" "$forbidden"
done

# exec should not import history
check_no_import "exec" "history"

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS module layer violation(s)"
  exit 1
fi

echo "PASS: Module dependency layers are correct"
exit 0
