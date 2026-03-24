#!/usr/bin/env bash
# Fuzz the placeholder resolution system with adversarial templates.
# Ensures resolve_placeholders never panics — only clean results.
set -euo pipefail

BINARY="./target/debug/baton"
if [[ ! -x "$BINARY" ]]; then
  cargo build --quiet 2>/dev/null
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

echo "Fuzzing placeholder resolution with adversarial templates..."

FAILURES=0

fuzz_case() {
  local name="$1"
  local prompt_content="$2"

  local config_dir="$TMPDIR/$name"
  mkdir -p "$config_dir/prompts"

  # Write adversarial prompt
  printf '%s' "$prompt_content" > "$config_dir/prompts/fuzz.md"

  # Write a minimal config that uses this prompt
  cat > "$config_dir/baton.toml" << 'EOF'
version = "0.7"

[runtimes.default]
type = "api"
base_url = "http://localhost:9999"
api_key_env = "BATON_FUZZ_KEY"
default_model = "test"

[validators.fuzz]
type = "llm"
runtime = "default"
prompt_file = "prompts/fuzz.md"

[gates.test]
validators = [
    { ref = "fuzz" },
]
EOF

  # Write a dummy input file
  echo "test content" > "$config_dir/input.txt"

  # doctor should not panic (it may error, but cleanly)
  if ! "$BINARY" doctor --offline --config "$config_dir/baton.toml" >/dev/null 2>&1; then
    : # Expected for some adversarial cases
  fi

  # dry-run should not panic
  local stderr_file="$TMPDIR/${name}_stderr.txt"
  "$BINARY" check --config "$config_dir/baton.toml" --dry-run "$config_dir/input.txt" >/dev/null 2>"$stderr_file" || true

  # Check for panic
  if grep -q 'panic\|RUST_BACKTRACE\|thread.*panicked' "$stderr_file" 2>/dev/null; then
    echo "PANIC in case '$name'!"
    cat "$stderr_file"
    FAILURES=$((FAILURES + 1))
  fi
}

# Nested braces
fuzz_case "nested-braces" '{{{{{file}}}}}'

# Empty placeholder
fuzz_case "empty-placeholder" '{}'

# Dot-only placeholder
fuzz_case "dot-only" '{.}'

# Double dot
fuzz_case "double-dot" '{input..path}'

# Trailing dot
fuzz_case "trailing-dot" '{input.}'

# Nonexistent verdict reference
fuzz_case "bad-verdict" '{verdict.nonexistent.status}'

# Empty verdict subpath
fuzz_case "empty-verdict" '{verdict..status}'

# Unclosed brace
fuzz_case "unclosed" 'Check this: {file'

# Huge template (100KB of repeated placeholder)
huge=$(python3 -c "print('{file} ' * 20000)" 2>/dev/null || printf '{file} %.0s' $(seq 1 20000))
fuzz_case "huge-template" "$huge"

# NUL bytes in template
fuzz_case "nul-bytes" "$(printf 'before\x00after {file}')"

# Nested placeholder-like patterns
fuzz_case "nested-pattern" '{file.{path}}'

# Unknown subpath
fuzz_case "unknown-subpath" '{file.nonexistent_property}'

# Verdict with unknown subpath
fuzz_case "verdict-unknown-sub" '{verdict.x.unknown_subpath}'

# Only opening braces
fuzz_case "only-opens" '{{{{{{{{{{{'

# Mixed valid and invalid
fuzz_case "mixed" '{file} then {bad.ref.deep.path} and {verdict.x.status}'

echo "Placeholder fuzz: all cases completed without panics"
if [[ $FAILURES -gt 0 ]]; then
  echo "FAIL: $FAILURES case(s) caused panics"
  exit 1
fi
exit 0
