# Testing Guide

## Running Tests

```bash
cargo test                                # All tests
cargo test config::tests                  # One module
cargo test config::tests::parse_minimal   # Single test (substring match)
cargo test -- --nocapture                 # Show println! output
cargo test -- --test-threads=1            # Sequential (for debugging shared state)
```

To get current per-module test counts:

```bash
cargo test -- --list 2>/dev/null | grep '::' | sed 's/::[^:]*$//' | sort | uniq -c | sort -rn
```

## Development Workflow

New features and bug fixes follow spec → tests → implementation order:

1. **Edit the spec first** — add or update `SPEC-XX-YY-NNN` assertions in the relevant `spec/*.md` file. Mark new assertions as `UNTESTED`.
2. **Write tests** — implement tests that exercise the new assertions. Update the spec to reference the test name.
3. **Write implementation** — make the tests pass.

The spec is the authoritative behavior reference. If the implementation disagrees with the spec, fix the implementation. This ordering keeps the spec, tests, and code in sync.

## Test Structure

Every module has `#[cfg(test)] mod tests` at the bottom. Tests are grouped by section dividers:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    // ─── Parsing ─────────────────────────────────────

    #[test]
    fn parse_minimal_config() { ... }

    // ─── Validation ──────────────────────────────────

    #[test]
    fn validate_missing_command() { ... }
}
```

## Runtime Adapter Tests

Session-based runtime adapters (OpenCode, OpenHands, and any future adapters) share identical HTTP lifecycle logic. To avoid duplicating ~40 tests per adapter, the shared test suite lives in a macro:

```rust
// src/runtime/session_common.rs defines the macro
session_adapter_tests!(AdapterType, "ENV_PREFIX", |server| { /* factory */ });
```

Each adapter file (`opencode.rs`, `openhands.rs`) invokes this macro in its `mod tests` block. The macro generates tests for: status mapping, cost extraction, constructor behavior, auth headers, and all HTTP operations (health check, create session, poll, collect, cancel, teardown).

**Adding a new session-based runtime adapter:**

1. Create `src/runtime/my_adapter.rs` with a struct wrapping `SessionAdapterBase`
2. Implement `RuntimeAdapter` by delegating all methods to `self.base.*`
3. Add a `#[cfg(test)] mod tests` block that invokes `session_adapter_tests!`
4. Wire it into `create_adapter()` in `src/runtime/mod.rs`

The macro gives you full test coverage automatically. Only add adapter-specific tests outside the macro if the adapter has unique behavior.

**Key files:**
- `src/runtime/session_common.rs` — `SessionAdapterBase` (shared impl) + `session_adapter_tests!` (shared tests) + `map_session_status()` + `extract_cost_from_metrics()`
- `src/runtime/opencode.rs` / `openhands.rs` — thin wrappers that delegate to `SessionAdapterBase`

## Patterns

### Filesystem Tests

Always use `tempfile` — never write to hard-coded paths:

```rust
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn input_file_lazy_content_loading() {
    let mut f = NamedTempFile::new().unwrap();
    write!(f, "hello").unwrap();
    let input = InputFile::new(f.path().to_path_buf());
    // content not loaded yet — lazy via OnceCell
    assert_eq!(input.get_content().unwrap(), "hello");
}
```

### Error Assertions

Prefer `is_err()` + `contains()` over `#[should_panic]`:

```rust
#[test]
fn rejects_nonexistent_input_file() {
    let input = InputFile::new(PathBuf::from("/nonexistent/file.txt"));
    let result = input.get_content();
    assert!(result.is_err());
}
```

### Config Test Helpers

Build TOML strings inline. Use raw string literals for readability:

```rust
let toml = r#"
version = "0.4"
[gates.review]
[[gates.review.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
let config = parse_config(toml, Path::new(".")).unwrap();
```

### Testing Validator Execution

`run_gate()` executes real subprocesses. Use simple shell commands (`echo`, `exit 1`) as validators in tests, not external tools:

```rust
let validator = ValidatorConfig {
    name: "always-pass".into(),
    validator_type: ValidatorType::Script,
    command: Some("echo PASS".into()),
    // ...
};
```

## What to Test

For new features, cover:

1. **Happy path** — the feature works as designed
2. **Error path** — invalid input produces a clear error (assert on message content)
3. **Edge cases** — empty input, missing optional fields, boundary values
4. **Interaction** — how the feature composes with existing features (e.g., `run_if` + `--skip`)

## Spec Files & Coverage Gaps

Each module has a corresponding spec file in `spec/` (e.g., `spec/exec.md`). These are detailed decision trees documenting every behavior as machine-readable `SPEC-XX-YY-NNN` assertions. See `docs/SPEC.md` for the full format guide, assertion ID conventions, and how to write new spec entries.

To find what still needs tests:

```bash
grep -r "UNTESTED" spec/          # All untested assertions
grep -c "UNTESTED" spec/*.md      # Count per module
```

When adding a new feature or fixing a bug, check the relevant spec file for `UNTESTED` assertions related to your change — these are pre-identified gaps worth covering.
