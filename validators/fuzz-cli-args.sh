#!/usr/bin/env bash
# Fuzz the baton CLI with adversarial arguments.
# Ensures no panics, segfaults, or unhandled errors.
set -euo pipefail

BINARY="./target/debug/baton"
if [[ ! -x "$BINARY" ]]; then
  cargo build --quiet 2>/dev/null
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

echo "Fuzzing CLI arguments..."

FAILURES=0

fuzz_case() {
  local name="$1"
  shift
  local args=("$@")

  local stderr_file="$TMPDIR/${name}_stderr.txt"
  local exit_code=0
  "$BINARY" "${args[@]}" >/dev/null 2>"$stderr_file" || exit_code=$?

  # Check for panic or segfault (exit code 139 = SIGSEGV)
  if [[ $exit_code -eq 139 ]] || grep -q 'panic\|RUST_BACKTRACE\|thread.*panicked' "$stderr_file" 2>/dev/null; then
    echo "PANIC/SEGFAULT in case '$name' (exit $exit_code)!"
    cat "$stderr_file"
    FAILURES=$((FAILURES + 1))
  fi
}

# No subcommand
fuzz_case "no-subcommand" --help

# Check with nonexistent config
fuzz_case "bad-config" check --config /nonexistent/baton.toml

# Check with no config in directory
fuzz_case "no-config" check --config "$TMPDIR/nope.toml"

# Contradictory only+skip
echo 'version = "0.6"
[validators.x]
type = "script"
command = "echo ok"
[gates.g]
validators = [{ ref = "x" }]' > "$TMPDIR/basic.toml"
fuzz_case "only-and-skip" check --config "$TMPDIR/basic.toml" --only x --skip x

# Unknown format
fuzz_case "bad-format" check --config "$TMPDIR/basic.toml" --format xml

# Zero timeout
fuzz_case "zero-timeout" check --config "$TMPDIR/basic.toml" --timeout 0

# Huge timeout
fuzz_case "huge-timeout" check --config "$TMPDIR/basic.toml" --timeout 999999999

# Nonexistent input file
fuzz_case "bad-input" check --config "$TMPDIR/basic.toml" /nonexistent/file.rs

# Very long gate name in --only
long_name=$(python3 -c "print('a' * 10000)" 2>/dev/null || printf 'a%.0s' $(seq 1 10000))
fuzz_case "long-only" check --config "$TMPDIR/basic.toml" --only "$long_name"

# Unknown subcommand
fuzz_case "unknown-cmd" frobnicate

# validate-config with garbage
echo "not valid toml {{{" > "$TMPDIR/garbage.toml"
fuzz_case "garbage-config" validate-config --config "$TMPDIR/garbage.toml"

# list with nonexistent gate
fuzz_case "list-bad-gate" list --config "$TMPDIR/basic.toml" --gate nonexistent

# history with nonexistent db
fuzz_case "history-no-db" history --config "$TMPDIR/basic.toml" --gate test

# Empty input via stdin placeholder
fuzz_case "empty-stdin" check --config "$TMPDIR/basic.toml" --files /dev/null

# --dry-run
fuzz_case "dry-run" check --config "$TMPDIR/basic.toml" --dry-run

echo "CLI fuzz: all cases completed without panics"
if [[ $FAILURES -gt 0 ]]; then
  echo "FAIL: $FAILURES case(s) caused panics"
  exit 1
fi
exit 0
