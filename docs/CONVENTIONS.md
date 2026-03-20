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
- Shared test helpers live in `src/test_helpers.rs` (`#[cfg(test)]` gated, `pub mod` in `lib.rs`). This module provides `ValidatorBuilder`, result/gate/config factories, `MockRuntimeAdapter`, and factories for `InputFile` and `Invocation`. Import as `use crate::test_helpers as th;`. If a helper is only needed by one module, keep it local to that module's `mod tests`.

## Placeholder Resolution Is Lazy

Files referenced by placeholders (`{file.content}`, `{input.spec.content}`) are read only when the placeholder is actually used via `InputFile`'s lazy loading. Never eagerly load content "in case" a validator needs it.

## CLI Argument Parsing

- Use clap derive macros, not the builder API.
- Subcommands are variants of a single `Commands` enum.
- Positional args are input files/directories; `--only`/`--skip` accept gate names, `gate.validator` dot paths, and `@tag` selectors.

## Version: Single Source of Truth

The version is defined once in `Cargo.toml` under `[package].version`. All runtime references use `env!("CARGO_PKG_VERSION")`. Never hardcode a version string in Rust source, README, or other files. The Homebrew formula version is updated by CI during releases.

## Spec-First Development

The `spec/*.md` files are the authoritative behavior reference. Each file is a detailed decision tree for its module, with every decision point, error return, and invariant documented as a `SPEC-XX-YY-NNN` assertion. New features and bug fixes follow this order:

1. **Edit the spec** — add or update assertions in the relevant `spec/*.md` file. Mark new assertions as `UNTESTED`.
2. **Write tests** — implement tests that exercise the new assertions. Update the assertion to reference the test name.
3. **Write implementation** — make the tests pass.

When modifying existing behavior, update the corresponding spec file to stay in sync. If the implementation disagrees with the spec, the implementation is wrong. See `docs/SPEC.md` for the full format guide and assertion ID conventions.

## Commit & PR Conventions

- Commits are imperative mood, lowercase: `add timeout enforcement`, `fix run_if with missing validator`.
- One logical change per commit. Config parsing changes and execution changes are separate commits even if related.
