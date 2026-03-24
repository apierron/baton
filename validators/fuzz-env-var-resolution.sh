#!/usr/bin/env bash
# Fuzz env var resolution with adversarial patterns.
# Ensures resolve_env_vars never panics on malformed ${...} patterns.
set -euo pipefail

BINARY="./target/debug/baton"
if [[ ! -x "$BINARY" ]]; then
  cargo build --quiet 2>/dev/null
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

echo "Fuzzing env var resolution..."

FAILURES=0

fuzz_case() {
  local name="$1"
  local env_pattern="$2"

  local config_file="$TMPDIR/${name}.toml"
  cat > "$config_file" << EOF
version = "0.7"

[validators.test]
type = "script"
command = "echo $env_pattern"

[gates.test]
validators = [
    { ref = "test" },
]
EOF

  local stderr_file="$TMPDIR/${name}_stderr.txt"
  "$BINARY" doctor --offline --config "$config_file" >/dev/null 2>"$stderr_file" || true

  if grep -q 'panic\|RUST_BACKTRACE\|thread.*panicked' "$stderr_file" 2>/dev/null; then
    echo "PANIC in case '$name'!"
    cat "$stderr_file"
    FAILURES=$((FAILURES + 1))
  fi
}

# Nonexistent env var (no default)
fuzz_case "nonexistent" '${BATON_FUZZ_NONEXISTENT_12345}'

# Empty default
fuzz_case "empty-default" '${BATON_FUZZ_NONEXISTENT_12345:-}'

# Nested env var reference
fuzz_case "nested" '${${HOME}}'

# Unclosed
fuzz_case "unclosed" '${BATON_FUZZ'

# Empty name
fuzz_case "empty-name" '${}'

# Empty name with default
fuzz_case "empty-name-default" '${:-fallback}'

# Shell injection in default
fuzz_case "injection-backtick" '${BATON_FUZZ_NONEXISTENT:-`echo pwned`}'
fuzz_case "injection-subshell" '${BATON_FUZZ_NONEXISTENT:-$(echo pwned)}'

# Very long default value
long_default=$(python3 -c "print('x' * 100000)" 2>/dev/null || printf 'x%.0s' $(seq 1 100000))
fuzz_case "huge-default" "\${BATON_FUZZ_NONEXISTENT:-$long_default}"

# Multiple env vars
fuzz_case "multiple" '${HOME} and ${PATH} and ${BATON_FUZZ_NONEXISTENT:-default}'

# Dollar without brace
fuzz_case "dollar-no-brace" '$HOME'

# Double dollar
fuzz_case "double-dollar" '$${HOME}'

# Brace without dollar
fuzz_case "brace-no-dollar" '{HOME}'

echo "Env var fuzz: all cases completed without panics"
if [[ $FAILURES -gt 0 ]]; then
  echo "FAIL: $FAILURES case(s) caused panics"
  exit 1
fi
exit 0
