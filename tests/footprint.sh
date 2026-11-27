#!/usr/bin/env bash
# Tests for validators/footprint.sh against a throwaway git repository.
# Usage: bash tests/footprint.sh            (BATON=/path/to/baton also runs it through `baton check`)
set -euo pipefail

script="$(cd "$(dirname "$0")/.." && pwd)/validators/footprint.sh"
repo=$(mktemp -d)
trap 'rm -rf "$repo"' EXIT
cd "$repo"
git init -q -b master
git config user.email test@example.com
git config user.name test
mkdir -p src tests work/items docs
printf 'fn main() {}\n' >src/main.rs

failures=0
card() { # name, header line(s)
  printf 'id: t-%s\ntitle: test\nstatus: open\n%s\n---\nbody\nfootprint: files=0\n' "$1" "$2" >"work/items/$1.md"
}
expect() { # description, expected exit code, expected output pattern, card [base]
  local desc="$1" code="$2" pattern="$3"
  shift 3
  local out rc=0
  out=$(bash "$script" "$@" 2>&1) || rc=$?
  if [[ $rc == "$code" ]] && grep -qE -- "$pattern" <<<"$out"; then
    echo "ok   $desc"
  else
    echo "FAIL $desc: exit $rc (want $code), output: $out"
    failures=$((failures + 1))
  fi
}
reset() { git reset -q --hard && git clean -qfd; }

card limits 'footprint: files=2 lines=10 paths=src/,tests/'
card none 'labels: x'
card bad 'footprint: files=five'
git add . && git commit -qm init

# Under the limit: two files, 3 lines. Tests and generated files are not counted.
printf 'a\nb\n' >src/a.rs
printf 'fn x() {}\n' >>src/main.rs
printf 'x\n%.0s' {1..50} >tests/big_test.rs
printf 'x\n%.0s' {1..50} >src/Cargo.lock
expect "under the limit passes" 0 '^PASS: 2 files, 3 lines' work/items/limits.md
reset

# Over the files limit, and the overrun names every counted file.
printf 'a\n' >src/a.rs
printf 'b\n' >src/b.rs
printf 'c\n' >src/c.rs
expect "over the files limit fails" 1 '^FAIL: 3 files, limit 2: src/a.rs, src/b.rs, src/c.rs$' work/items/limits.md
reset

# Over the lines limit (committed on a branch, measured against the base).
git checkout -qb feature
printf 'x\n%.0s' {1..11} >src/a.rs
git add src/a.rs && git commit -qm big
expect "over the lines limit fails" 1 '^FAIL: 11 lines changed, limit 10$' work/items/limits.md master
expect "base defaults to HEAD" 0 '^PASS: 0 files, 0 lines' work/items/limits.md
git checkout -q master
reset

# A path outside the allowed list fails, even for a test file.
mkdir -p docs && printf 'd\n' >docs/notes.md
mkdir -p other/tests && printf 't\n' >other/tests/x.rs
expect "path outside the allowed list fails" 1 '^FAIL: 2 files outside allowed paths \(src/,tests/\): docs/notes.md, other/tests/x.rs$' work/items/limits.md
reset

# No footprint line in the header (the one in the body does not count): skipped with a message.
printf 'a\n' >src/a.rs
expect "card with no footprint line is skipped" 0 '^SKIP: work/items/none.md has no footprint: line$' work/items/none.md
reset

expect "missing card is a usage error" 2 'work item file not found' work/items/missing.md
expect "bad number is a usage error" 2 "'five' is not a whole number" work/items/bad.md

# Through baton itself, when a binary is given.
if [[ -n "${BATON:-}" ]]; then
  cat >baton.toml <<EOF
version = "0.7"
[validators.footprint]
type = "script"
command = "bash $script {file.path}"
input = "work/items/*.md"
[gates.footprint]
validators = [{ ref = "footprint", blocking = true }]
EOF
  git add baton.toml && git commit -qm config
  printf 'a\n' >src/a.rs
  rc=0
  "$BATON" check --no-log --format summary work/items/limits.md >/dev/null 2>&1 || rc=$?
  [[ $rc == 0 ]] && echo "ok   baton check passes under the limit" || { echo "FAIL baton check under the limit: exit $rc"; failures=$((failures + 1)); }
  printf 'b\n' >src/b.rs
  printf 'c\n' >src/c.rs
  rc=0
  "$BATON" check --no-log --format summary work/items/limits.md >/dev/null 2>&1 || rc=$?
  [[ $rc == 1 ]] && echo "ok   baton check fails over the limit" || { echo "FAIL baton check over the limit: exit $rc"; failures=$((failures + 1)); }
fi

if [[ $failures -gt 0 ]]; then
  echo "$failures footprint test(s) failed"
  exit 1
fi
echo "All footprint tests passed"
