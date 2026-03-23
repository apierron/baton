#!/usr/bin/env bash
# Invariant: two-stage config design must be preserved.
# parse_config must not do semantic validation.
# validate_config must not do TOML deserialization.
# Per ARCHITECTURE.md and BOUNDARIES.md.
set -euo pipefail

echo "Checking invariant: two-stage config separation..."

CONFIG_FILE="src/config.rs"
if [[ ! -f "$CONFIG_FILE" ]]; then
  echo "ERROR: $CONFIG_FILE not found"
  exit 2
fi

VIOLATIONS=0

# Extract parse_config function body (from "pub fn parse_config" to next "pub fn" or EOF)
# Check it does not construct ValidationError
parse_body=$(sed -n '/^pub fn parse_config/,/^pub fn /p' "$CONFIG_FILE" | sed '$d')
validation_in_parse=$(echo "$parse_body" | grep -n 'ValidationError' || true)
if [[ -n "$validation_in_parse" ]]; then
  while IFS= read -r match; do
    echo "VIOLATION: parse_config constructs ValidationError: $match"
    VIOLATIONS=$((VIOLATIONS + 1))
  done <<< "$validation_in_parse"
fi

# Extract validate_config function body
# Check it does not call toml::from_str or use serde deserialization
validate_body=$(sed -n '/^pub fn validate_config/,/^pub fn /p' "$CONFIG_FILE" | sed '$d')
toml_in_validate=$(echo "$validate_body" | grep -n 'toml::from_str\|toml::de\|Deserialize' || true)
if [[ -n "$toml_in_validate" ]]; then
  while IFS= read -r match; do
    echo "VIOLATION: validate_config does TOML deserialization: $match"
    VIOLATIONS=$((VIOLATIONS + 1))
  done <<< "$toml_in_validate"
fi

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Two-stage config design violated. parse_config and validate_config must stay separate."
  exit 1
fi

echo "PASS: Two-stage config design is intact"
exit 0
