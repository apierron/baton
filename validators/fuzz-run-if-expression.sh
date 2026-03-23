#!/usr/bin/env bash
# Fuzz the run_if expression parser with malformed inputs.
# Ensures validate-config never panics on adversarial run_if values.
set -euo pipefail

BINARY="./target/debug/baton"
if [[ ! -x "$BINARY" ]]; then
  cargo build --quiet 2>/dev/null
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

echo "Fuzzing run_if expression parser..."

FAILURES=0

fuzz_case() {
  local name="$1"
  local run_if_value="$2"

  local config_file="$TMPDIR/${name}.toml"
  cat > "$config_file" << EOF
version = "0.6"

[validators.a]
type = "script"
command = "echo ok"

[validators.b]
type = "script"
command = "echo ok"
run_if = "$run_if_value"

[gates.test]
validators = [
    { ref = "a" },
    { ref = "b" },
]
EOF

  local stderr_file="$TMPDIR/${name}_stderr.txt"
  "$BINARY" validate-config --config "$config_file" >/dev/null 2>"$stderr_file" || true

  if grep -q 'panic\|RUST_BACKTRACE\|thread.*panicked' "$stderr_file" 2>/dev/null; then
    echo "PANIC in case '$name'!"
    cat "$stderr_file"
    FAILURES=$((FAILURES + 1))
  fi
}

# Trailing operator
fuzz_case "trailing-and" "a.status == pass and"

# Empty string
fuzz_case "empty" ""

# Unknown status value
fuzz_case "unknown-status" "a.status == banana"

# Missing dot-status
fuzz_case "no-dot-status" "a == pass"

# Double operator
fuzz_case "double-and" "a.status == pass and and b.status == pass"

# Double or
fuzz_case "double-or" "a.status == pass or or b.status == pass"

# Just an operator
fuzz_case "just-and" "and"

# Just a status
fuzz_case "just-pass" "pass"

# Unicode validator name
fuzz_case "unicode-name" "émoji.status == pass"

# Very long chain
long_chain=""
for i in $(seq 1 50); do
  if [[ -n "$long_chain" ]]; then
    long_chain="$long_chain and "
  fi
  long_chain="${long_chain}a.status == pass"
done
fuzz_case "long-chain" "$long_chain"

# Reference to self
fuzz_case "self-ref" "b.status == pass"

# Nonexistent validator
fuzz_case "nonexistent" "zzz.status == pass"

# Comparison without value
fuzz_case "no-value" "a.status =="

# Wrong comparison operator
fuzz_case "wrong-op" "a.status != pass"

# Nested parens (if supported)
fuzz_case "parens" "(a.status == pass)"

# Leading whitespace
fuzz_case "leading-space" "  a.status == pass"

# Trailing whitespace
fuzz_case "trailing-space" "a.status == pass  "

echo "Run-if fuzz: all cases completed without panics"
if [[ $FAILURES -gt 0 ]]; then
  echo "FAIL: $FAILURES case(s) caused panics"
  exit 1
fi
exit 0
