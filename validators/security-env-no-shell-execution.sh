#!/usr/bin/env bash
# Security: env var resolution must not execute shell commands.
# Per BOUNDARIES.md: "Environment variable interpolation (${VAR}) in config
# must not execute shell commands."
# This is a tripwire — if someone adds shell execution to placeholder.rs, it fires.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "Checking security: no shell execution in env var resolution..."

VIOLATIONS=0

code=$(prod_code "src/placeholder.rs")

# Check for any shell/command execution patterns
for pattern in 'Command::new' 'process::Command' 'std::process' 'sh -c' 'bash -c' '.spawn()' '.output()'; do
  matches=$(echo "$code" | grep -n "$pattern" || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      echo "VIOLATION: src/placeholder.rs: shell execution in env var module: $match"
      VIOLATIONS=$((VIOLATIONS + 1))
    done <<< "$matches"
  fi
done

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: placeholder.rs contains shell execution patterns. Env var resolution must use std::env::var() only."
  exit 1
fi

echo "PASS: No shell execution in env var resolution"
exit 0
