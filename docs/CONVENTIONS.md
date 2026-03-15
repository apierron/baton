# Conventions

Mechanical rules for keeping the codebase consistent. These are the "golden principles" — when in doubt, follow these rather than inventing a new pattern.

## Deterministic Output

Use `BTreeMap` instead of `HashMap` whenever the map's contents affect output or hashing. This ensures that JSON output, SHA-256 context hashes, and test assertions are stable across runs.

## Validate at Boundaries, Trust Internally

- Validate user-supplied input at the CLI and config parsing boundaries.
- Internal functions receive already-validated data and use `expect()` for invariants that would indicate a bug, not user error.
- Never re-validate what `validate_config()` already checked inside `run_gate()`.

## Error Messages Are User-Facing

Every `BatonError` variant's `#[error("...")]` string may be shown to the end user. Include enough context to act on:

```rust
// Good: user knows what to fix
#[error("Gate not found: '{name}'. Available gates: {available}")]

// Bad: user has to guess
#[error("Gate not found")]
```

## Prefer Shared Types Over Ad-Hoc Structs

If two modules need the same shape of data, put the type in `types.rs`. Don't create module-local structs that mirror types already in `types.rs`.

## Config Defaults Live Next to Raw Structs

Serde default functions (`fn default_timeout() -> u64 { 300 }`) live immediately below the struct they serve in `config.rs`. Don't scatter defaults across the file.

## Tests Mirror the Code They Test

- Tests for `config.rs` live in `config.rs`, not in a separate `tests/` directory.
- Integration tests (testing the compiled binary) use `assert_cmd` and live in `tests/`.
- Test helper functions are defined inside `mod tests`, not extracted to a shared test utils module — unless three or more test modules need them.

## Placeholder Resolution Is Lazy

Files referenced by placeholders (`{artifact_content}`, `{context.spec.content}`) are read only when the placeholder is actually used. Never eagerly load content "in case" a validator needs it.

## CLI Argument Parsing

- Use clap derive macros, not the builder API.
- Subcommands are variants of a single `Commands` enum.
- Repeated key-value args (like `--context name=path`) use a custom `value_parser` function.

## Version: Single Source of Truth

The version is defined once in `Cargo.toml` under `[package].version`. All runtime references use `env!("CARGO_PKG_VERSION")`. Never hardcode a version string in Rust source, README, or other files. The Homebrew formula version is updated by CI during releases.

## Spec Assertions

When adding new behavior or modifying existing behavior, update the corresponding `spec/*.md` file. Each assertion (`SPEC-XX-YY-NNN`) should map to a test — mark new assertions as `UNTESTED` until a test exists. See `docs/TESTING.md` for details.

## Commit & PR Conventions

- Commits are imperative mood, lowercase: `add timeout enforcement`, `fix run_if with missing validator`.
- One logical change per commit. Config parsing changes and execution changes are separate commits even if related.
