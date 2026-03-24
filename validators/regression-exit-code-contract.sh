#!/usr/bin/env bash
# Regression guard: exit code contract.
# baton check must return 0 on pass, 1 on fail, 2 on error.
set -euo pipefail

BINARY="./target/debug/baton"
if [[ ! -x "$BINARY" ]]; then
  cargo build --quiet 2>/dev/null
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

echo "Checking regression: exit code contract..."

FAILURES=0

# Case 1: passing config → exit 0
cat > "$TMPDIR/pass.toml" << 'EOF'
version = "0.7"
[validators.ok]
type = "script"
command = "echo PASS"
[gates.test]
validators = [{ ref = "ok" }]
EOF

exit_code=0
"$BINARY" check --config "$TMPDIR/pass.toml" --no-log >/dev/null 2>&1 || exit_code=$?
if [[ $exit_code -ne 0 ]]; then
  echo "FAIL: passing config exited $exit_code, expected 0"
  FAILURES=$((FAILURES + 1))
else
  echo "  pass case: exit $exit_code (expected 0) ✓"
fi

# Case 2: failing validator → exit 1
cat > "$TMPDIR/fail.toml" << 'EOF'
version = "0.7"
[validators.bad]
type = "script"
command = "echo FAIL && exit 1"
[gates.test]
validators = [{ ref = "bad", blocking = true }]
EOF

exit_code=0
"$BINARY" check --config "$TMPDIR/fail.toml" --no-log >/dev/null 2>&1 || exit_code=$?
if [[ $exit_code -ne 1 ]]; then
  echo "FAIL: failing config exited $exit_code, expected 1"
  FAILURES=$((FAILURES + 1))
else
  echo "  fail case: exit $exit_code (expected 1) ✓"
fi

# Case 3: broken config → exit 2
cat > "$TMPDIR/error.toml" << 'EOF'
this is not valid toml at all {{{
EOF

exit_code=0
"$BINARY" check --config "$TMPDIR/error.toml" --no-log >/dev/null 2>&1 || exit_code=$?
if [[ $exit_code -ne 2 ]]; then
  echo "FAIL: broken config exited $exit_code, expected 2"
  FAILURES=$((FAILURES + 1))
else
  echo "  error case: exit $exit_code (expected 2) ✓"
fi

if [[ $FAILURES -gt 0 ]]; then
  echo "FAIL: $FAILURES exit code contract violation(s)"
  exit 1
fi

echo "PASS: Exit code contract holds (0/1/2)"
exit 0
