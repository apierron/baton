#!/usr/bin/env bash
# Footprint gate: fail a git diff that is bigger than its work item says it may be.
#
# Usage: footprint.sh <work-item.md> [base-ref]
#
# The work item (a markdown file with `key: value` header lines, ended by the first `---` line)
# declares its size in a header line:
#   footprint: files=5 lines=400 paths=src/,tests/
# Every key is optional. files and lines are maximums; paths is a comma-separated list of
# allowed path prefixes.
#
# The diff is the working tree (including untracked files) against the merge-base of
# base-ref and HEAD. base-ref defaults to $FOOTPRINT_BASE, else HEAD (uncommitted changes).
#
# Counting:
#   - lines = additions + deletions from `git diff --numstat` (an untracked file counts its lines).
#   - Tests (a tests/, test/ or __tests__/ directory, or *_test.*, *_tests.*, test_*, *.test.*,
#     *.spec.*) are left out of the files and lines counts, but must still be inside `paths`.
#   - Generated files (lockfiles, or marked `linguist-generated` in .gitattributes) are left
#     out of every check.
#
# Exit 0: within the footprint, or the item has no footprint line (prints a skip message).
# Exit 1: over a limit; every overrun is named. Exit 2: usage or git error.
set -euo pipefail

card="${1:-}"
base="${2:-${FOOTPRINT_BASE:-HEAD}}"

if [[ -z "$card" || ! -f "$card" ]]; then
  echo "footprint: work item file not found: '${card}'" >&2
  exit 2
fi

# The footprint line lives in the header, which ends at the first line that is exactly `---`.
line=$(sed '/^---$/q' "$card" | grep -m1 '^footprint:' || true)
if [[ -z "$line" ]]; then
  echo "SKIP: $card has no footprint: line"
  exit 0
fi

# A whole number, read as decimal (bash reads a leading 0 as octal).
num() {
  [[ "$1" =~ ^[0-9]+$ ]] || { echo "footprint: '$1' is not a whole number in $card: $line" >&2; exit 2; }
  echo $((10#$1))
}
max_files="" max_lines="" paths=""
for kv in ${line#footprint:}; do
  case "$kv" in
    files=*) max_files=$(num "${kv#files=}") ;;
    lines=*) max_lines=$(num "${kv#lines=}") ;;
    paths=*) paths="${kv#paths=}" ;;
    *) echo "footprint: unknown field '$kv' in $card (expected files=N lines=N paths=a/,b/)" >&2; exit 2 ;;
  esac
done

if ! merge_base=$(git merge-base "$base" HEAD 2>/dev/null); then
  echo "footprint: cannot find a merge-base of '$base' and HEAD" >&2
  exit 2
fi
# git paths are repo-root relative; work from the root so untracked files match them.
cd "$(git rev-parse --show-toplevel)"

is_generated() {
  case "${1##*/}" in
    Cargo.lock | package-lock.json | yarn.lock | pnpm-lock.yaml | poetry.lock | uv.lock | Gemfile.lock | go.sum) return 0 ;;
  esac
  case "$(git check-attr linguist-generated -- "$1" | sed 's/.*: //')" in true | set) return 0 ;; esac
  return 1
}

is_test() {
  case "/$1" in
    */tests/* | */test/* | */__tests__/*) return 0 ;;
  esac
  case "${1##*/}" in
    *_test.* | *_tests.* | test_* | *.test.* | *.spec.*) return 0 ;;
  esac
  return 1
}

counted=() outside=()
total_lines=0

consider() { # path added deleted
  local path="$1" add="$2" del="$3"
  is_generated "$path" && return 0
  if [[ -n "$paths" ]]; then
    local ok=0 prefix
    IFS=',' read -r -a prefixes <<<"$paths"
    for prefix in "${prefixes[@]}"; do
      [[ "$path" == "$prefix"* ]] && ok=1
    done
    [[ $ok == 1 ]] || outside+=("$path")
  fi
  is_test "$path" && return 0
  counted+=("$path")
  # Binary files show "-" in numstat: they count as a file, not as lines.
  [[ "$add" == "-" ]] || total_lines=$((total_lines + add + del))
}

# Collect into files first: a git failure inside `< <(...)` would be silently ignored.
changes=$(mktemp) untracked=$(mktemp)
trap 'rm -f "$changes" "$untracked"' EXIT
git_or_die() { "$@" || { echo "footprint: '$*' failed" >&2; exit 2; }; }
# ponytail: --no-renames counts a rename as a delete plus an add; parse -M records if that bites.
git_or_die git diff --numstat --no-renames -z "$merge_base" >"$changes"
git_or_die git ls-files -z --others --exclude-standard >"$untracked"

while IFS=$'\t' read -r -d '' add del path; do
  consider "$path" "$add" "$del"
done <"$changes"

# An untracked file counts its lines the way git numstat would (exit 1 means "differs").
while IFS= read -r -d '' path; do
  IFS=$'\t' read -r add del _ < <(git diff --no-index --numstat -- /dev/null "$path" || [[ $? == 1 ]])
  consider "$path" "$add" "$del"
done <"$untracked"

join() { local out="" p; for p in "$@"; do out+="${out:+, }$p"; done; echo "$out"; }

fail=0
if [[ -n "$max_files" && ${#counted[@]} -gt $max_files ]]; then
  echo "FAIL: ${#counted[@]} files, limit $max_files: $(join "${counted[@]}")"
  fail=1
fi
if [[ -n "$max_lines" && $total_lines -gt $max_lines ]]; then
  echo "FAIL: $total_lines lines changed, limit $max_lines"
  fail=1
fi
if [[ ${#outside[@]} -gt 0 ]]; then
  echo "FAIL: ${#outside[@]} files outside allowed paths ($paths): $(join "${outside[@]}")"
  fail=1
fi

if [[ $fail == 0 ]]; then
  echo "PASS: ${#counted[@]} files, $total_lines lines (limits: files=${max_files:-none} lines=${max_lines:-none} paths=${paths:-any})"
fi
exit $fail
