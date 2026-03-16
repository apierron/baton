# Testing Guide

## Running Tests

```bash
cargo test                                # All tests
cargo test config::tests                  # One module
cargo test config::tests::parse_minimal   # Single test (substring match)
cargo test -- --nocapture                 # Show println! output
cargo test -- --test-threads=1            # Sequential (for debugging shared state)
```

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

## Patterns

### Filesystem Tests

Always use `tempfile` — never write to hard-coded paths:

```rust
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn artifact_lazy_content_loading() {
    let mut f = NamedTempFile::new().unwrap();
    write!(f, "hello").unwrap();
    let mut art = Artifact::from_file(f.path()).unwrap();
    // content not loaded yet
    assert_eq!(art.get_content_as_string().unwrap(), "hello");
}
```

### Error Assertions

Prefer `is_err()` + `contains()` over `#[should_panic]`:

```rust
#[test]
fn rejects_directory_as_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let result = Artifact::from_file(dir.path());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("directory"));
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

## Current Test Counts

| Module | Tests |
| ------ | ----- |
| types | 51 |
| verdict_parser | 35 |
| prompt | 28 |
| placeholder | 35 |
| config | 55 |
| exec | 116 |
| history | 34 |
| runtime | 34 |
| cli (integration) | 73 |
| **Total** | **461** |

## Spec Files & Coverage Gaps

Each module has a corresponding spec file in `spec/` (e.g., `spec/exec.md`) containing assertions in the format `SPEC-XX-YY-NNN`. Every assertion maps to an existing test (`test: module::tests::test_name`) or is marked `UNTESTED`.

To find what still needs tests:

```bash
grep -r "UNTESTED" spec/          # All untested assertions
grep -c "UNTESTED" spec/*.md      # Count per module
```

When adding a new feature or fixing a bug, check the relevant spec file for `UNTESTED` assertions related to your change — these are pre-identified gaps worth covering.
