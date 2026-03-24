#!/usr/bin/env bash
# Spec sync: verify that test references in spec files point to existing test functions.
# Catches stale references after test renames.
set -euo pipefail

echo "Checking spec sync: test references exist..."

VIOLATIONS=0
CHECKED=0

resolve_source_file() {
  local ref="$1"

  # Integration tests: cli::fn_name → tests/cli.rs
  if [[ "$ref" == cli::* ]]; then
    echo "tests/cli.rs"
    return
  fi

  # Integration tests: e2e::fn_name → tests/e2e.rs
  if [[ "$ref" == e2e::* ]]; then
    echo "tests/e2e.rs"
    return
  fi

  # Nested modules: runtime::api::tests::fn → src/runtime/api.rs
  # runtime::openhands::tests::fn → src/runtime/openhands.rs
  # runtime::opencode::tests::fn → src/runtime/opencode.rs
  # runtime::session_common::tests::fn → src/runtime/session_common.rs
  if [[ "$ref" == runtime::*::tests::* ]]; then
    local submod
    submod=$(echo "$ref" | sed 's/runtime::\([^:]*\)::tests::.*/\1/')
    echo "src/runtime/${submod}.rs"
    return
  fi

  # runtime::tests::fn → src/runtime/mod.rs
  if [[ "$ref" == runtime::tests::* ]]; then
    echo "src/runtime/mod.rs"
    return
  fi

  # commands submodules: add::tests::fn → src/commands/add.rs
  if [[ "$ref" == add::tests::* ]]; then
    echo "src/commands/add.rs"
    return
  fi

  # exec submodules: exec::tests::fn → search all src/exec/*.rs
  # Return a sentinel; caller will search the directory.
  if [[ "$ref" == exec::tests::* ]]; then
    echo "src/exec/*.rs"
    return
  fi

  # Standard: module::tests::fn → src/module.rs
  local module
  module=$(echo "$ref" | sed 's/::tests::.*//')
  echo "src/${module}.rs"
}

extract_fn_name() {
  local ref="$1"

  # cli::fn_name (no ::tests:: for integration tests)
  if [[ "$ref" == cli::* ]]; then
    echo "$ref" | sed 's/cli:://'
    return
  fi
  if [[ "$ref" == e2e::* ]]; then
    echo "$ref" | sed 's/e2e:://'
    return
  fi

  # module::tests::fn_name
  echo "$ref" | sed 's/.*::tests:://'
}

for spec_file in spec/*.md; do
  [[ -f "$spec_file" ]] || continue

  while IFS= read -r line; do
    [[ -z "$line" ]] && continue

    # Extract test reference, trimming whitespace
    ref=$(echo "$line" | sed 's/.*test: *//' | sed 's/ *$//')

    # Skip non-code references (comments, descriptions, placeholders)
    [[ "$ref" == IMPLICIT* ]] && continue
    [[ "$ref" == TODO* ]] && continue
    [[ "$ref" == MANUAL* ]] && continue
    [[ "$ref" == *"("* ]] && ref=$(echo "$ref" | sed 's/ *(.*//')
    [[ -z "$ref" ]] && continue

    CHECKED=$((CHECKED + 1))

    source_file=$(resolve_source_file "$ref")
    fn_name=$(extract_fn_name "$ref")

    # Handle glob patterns (e.g. src/exec/*.rs) — search all matching files
    if [[ "$source_file" == *'*'* ]]; then
      found=false
      for f in $source_file; do
        if [[ -f "$f" ]] && grep -q "fn ${fn_name}" "$f"; then
          found=true
          break
        fi
      done
      if [[ "$found" != true ]]; then
        echo "VIOLATION: $spec_file: test ref '$ref' — function '$fn_name' not found in $source_file"
        VIOLATIONS=$((VIOLATIONS + 1))
      fi
      continue
    fi

    if [[ ! -f "$source_file" ]]; then
      echo "VIOLATION: $spec_file: test ref '$ref' — source file '$source_file' not found"
      VIOLATIONS=$((VIOLATIONS + 1))
      continue
    fi

    if ! grep -q "fn ${fn_name}" "$source_file"; then
      echo "VIOLATION: $spec_file: test ref '$ref' — function '$fn_name' not found in $source_file"
      VIOLATIONS=$((VIOLATIONS + 1))
    fi
  done < <(grep '  test: ' "$spec_file" | grep -v 'UNTESTED' || true)
done

echo "Checked $CHECKED test references"

if [[ $VIOLATIONS -gt 0 ]]; then
  echo "FAIL: Found $VIOLATIONS stale test reference(s) in spec files"
  exit 1
fi

echo "PASS: All test references in spec files point to existing functions"
exit 0
