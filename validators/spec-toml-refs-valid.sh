#!/usr/bin/env bash
# Spec sync: validate that file references in baton.toml actually exist.
# Checks validator script paths and prompts directory.
set -euo pipefail

echo "Checking spec sync: baton.toml file references..."

CONFIG_FILE="baton.toml"
if [[ ! -f "$CONFIG_FILE" ]]; then
  echo "ERROR: $CONFIG_FILE not found"
  exit 2
fi

VIOLATIONS=0

# Extract command values that reference files
# Pattern: command = "bash validators/X.sh" or command = "bash path/to/script.sh"
while IFS= read -r line; do
  [[ -z "$line" ]] && continue

  # Extract the file path from bash/sh commands
  script_path=$(echo "$line" | grep -oE '(bash|sh) [^ "]+\.sh' | awk '{print $2}' || true)
  if [[ -n "$script_path" && ! -f "$script_path" ]]; then
    echo "VIOLATION: $CONFIG_FILE: references non-existent script: $script_path"
    VIOLATIONS=$((VIOLATIONS + 1))
  fi
done < <(grep 'command' "$CONFIG_FILE" || true)

# Check prompts_dir exists
prompts_dir=$(grep 'prompts_dir' "$CONFIG_FILE" | sed 's/.*= *"\(.*\)"/\1/' | head -1)
if [[ -n "$prompts_dir" && ! -d "$prompts_dir" ]]; then
  echo "VIOLATION: $CONFIG_FILE: prompts_dir '$prompts_dir' does not exist"
  VIOLATIONS=$((VIOLATIONS + 1))
fi

# Check prompt_file references
while IFS= read -r line; do
  [[ -z "$line" ]] && continue
  prompt_file=$(echo "$line" | sed 's/.*= *"\(.*\)"/\1/')
  # Resolve relative to prompts_dir
  if [[ -n "$prompts_dir" ]]; then
    full_path="${prompts_dir}/${prompt_file}"
  else
    full_path="./prompts/${prompt_file}"
  fi
  if [[ ! -f "$full_path" ]]; then
    echo "VIOLATION: $CONFIG_FILE: references non-existent prompt file: $full_path"
    VIOLATIONS=$((VIOLATIONS + 1))
  fi
done < <(grep 'prompt_file' "$CONFIG_FILE" || true)

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS broken file reference(s) in baton.toml"
  exit 1
fi

echo "PASS: All file references in baton.toml are valid"
exit 0
