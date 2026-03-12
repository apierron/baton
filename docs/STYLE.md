# Rust Style Guide

Code style conventions for the baton codebase. These supplement `rustfmt` and `clippy` — they cover decisions those tools don't enforce.

## Module Layout

Each module follows this order:

```rust
use external_crate::...;    // 1. External crate imports
use std::...;               // 2. Std imports
                            //
use crate::...;             // 3. Internal imports (blank line above)

// ─── Section Name ──────────────────────────────────  // 4. Section dividers

pub struct/fn ...           // 5. Public items first
fn private_fn ...           // 6. Private items after

#[cfg(test)]                // 7. Tests at bottom of file
mod tests {
    use super::*;
```

## Naming

- **Types:** `PascalCase`. Suffix enums with their role when ambiguous: `VerdictStatus` vs `Status`.
- **Functions:** `snake_case`. Prefix with verb: `parse_config`, `run_gate`, `evaluate_run_if`.
- **Builder/factory fns:** `from_*` for constructors (`from_file`, `from_string`, `from_bytes`).
- **Lazy accessors:** `get_*` when the first call loads/computes and caches (`get_content`, `get_hash`).
- **Test functions:** `snake_case` describing the scenario, not `test_` prefix: `artifact_from_file_not_found`, `parse_minimal_config`.

## Error Handling

- Use `crate::error::Result<T>` everywhere, never `std::result::Result<_, BatonError>` directly.
- Add new variants to `BatonError` rather than using `ConfigError(String)` or `ValidationError(String)` for genuinely distinct error classes.
- Error messages: include the offending value. Good: `"Gate not found: 'deploy'. Available gates: build, test"`. Bad: `"Gate not found"`.
- Convert external errors with `#[from]` when there's a 1:1 mapping, `map_err` when context is needed.

## Structs & Enums

- Derive order: `Debug, Clone, Serialize, Deserialize` (in that order, skip unused derives).
- Use `BTreeMap` over `HashMap` when ordering matters for deterministic output/hashing.
- Prefer `Option<T>` fields over sentinel values.
- Lazy-loaded fields: store as `Option<T>`, private, with a `get_*(&mut self)` accessor.

## Section Dividers

Use this format to separate logical sections within a file:

```rust
// ─── Section Name ──────────────────────────────────
```

Use these in both implementation code and test modules. They help agents navigate large files.

## Tests

- All tests live in `#[cfg(test)] mod tests` at the bottom of the module they test.
- Use `tempfile::NamedTempFile` or `tempfile::TempDir` for filesystem tests — never write to fixed paths.
- Group tests by concern using section dividers.
- Test the error path, not just the happy path. Assert on error message content with `contains()`.
- Prefer `assert!(result.is_err())` + inspecting the error over `#[should_panic]`.

```rust
#[test]
fn artifact_from_file_not_found() {
    let result = Artifact::from_file("/nonexistent/file.txt");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not found"), "Error: {err}");
}
```

## Formatting

- `cargo fmt` is authoritative. Do not fight it.
- One-line closures and match arms are fine when they fit on a single line.
- Default functions for serde: use standalone `fn default_*() -> T` functions, not const expressions.

```rust
#[serde(default = "default_timeout")]
pub timeout_seconds: u64,

fn default_timeout() -> u64 { 300 }
```

## Dependencies

- Prefer `features = ["bundled"]` for C libraries (e.g., rusqlite) so the build is self-contained.
- Pin to major version only (`"1"`, `"0.12"`) in Cargo.toml — let the lockfile handle exact versions.
- Dev-only crates go in `[dev-dependencies]`.
