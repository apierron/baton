#!/usr/bin/env bash
# Security: no hardcoded secrets, API keys, or tokens in source files.
set -euo pipefail

echo "Checking security: no secrets in source..."

VIOLATIONS=0

# Patterns that look like real secrets (not env var references or test data)
check_pattern() {
  local pattern="$1"
  local desc="$2"

  matches=$(grep -rnE "$pattern" src/ validators/ prompts/ baton.toml 2>/dev/null \
    | grep -v 'test_helpers.rs' \
    | grep -vE '^\s*(//|#).*example|dummy|fake|test|placeholder' \
    || true)

  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      echo "VIOLATION ($desc): $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi
}

# OpenAI-style secret keys
check_pattern 'sk-[a-zA-Z0-9]{20,}' "OpenAI API key"

# GitHub personal access tokens
check_pattern 'ghp_[a-zA-Z0-9]{36}' "GitHub PAT"

# AWS access key IDs
check_pattern 'AKIA[A-Z0-9]{16}' "AWS access key"

# Anthropic API keys
check_pattern 'sk-ant-[a-zA-Z0-9]{20,}' "Anthropic API key"

# Hardcoded password/secret/token assignments with literal values (not env vars)
check_pattern '(password|secret|token)\s*=\s*"[^${}][^"]{8,}"' "hardcoded credential"

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS potential secret(s) in source files."
  exit 1
fi

echo "PASS: No secrets detected in source files"
exit 0
