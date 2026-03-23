#!/usr/bin/env bash
# Shared helper functions for baton validators.
# Source this file: source "$(dirname "$0")/lib.sh"

# Extract production code from a Rust source file (strips #[cfg(test)] to EOF).
# Usage: prod_code file.rs
prod_code() {
  local file="$1"
  sed '/#\[cfg(test)\]/,$d' "$file" | grep -vE '^\s*(//|/\*|\*)' || true
}

# List all production .rs files (includes src/runtime/*.rs, excludes test_helpers.rs).
# Usage: all_rs_files
all_rs_files() {
  find src -name '*.rs' -not -name 'test_helpers.rs' | sort
}

# List all library .rs files (excludes main.rs and test_helpers.rs).
# Usage: lib_rs_files
lib_rs_files() {
  find src -name '*.rs' -not -name 'test_helpers.rs' -not -name 'main.rs' | sort
}
