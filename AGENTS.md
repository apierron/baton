# AGENTS.md

Instructions for AI coding agents working in this repository.

## What is Baton?

A composable validation gate for AI agent outputs. Accepts an artifact (file to validate) + context (reference docs), runs validators (script/LLM/human), produces a structured verdict (pass/fail/error), and persists results in SQLite.

## Commands

```bash
cargo build                          # Build
cargo test                           # Run all tests (461 across 8 modules + integration)
cargo test config::tests::test_name  # Run a single test
cargo test config::tests             # Run one module's tests
cargo clippy --all-targets -- -D warnings  # Lint (must pass with zero warnings)
cargo fmt --check # Formatting
```

## Architecture

**Data flow:** CLI → config discovery/parse/validate → load artifact & context → `run_gate()` → store verdict in SQLite → output (JSON/human/summary)

**Module dependency layers** (top → bottom, never import upward):

```text
main.rs → exec, config, history, runtime, types
exec → config, types, placeholder, runtime, error
config → types, placeholder, error
history, placeholder → types, error
runtime → types, error
prompt, verdict_parser → types or error only
error → (leaf, no internal imports)
```

See `docs/ARCHITECTURE.md` for the full dependency table and design rationale (two-stage config, lazy loading, Status vs VerdictStatus).

## Key Conventions

- `BTreeMap` over `HashMap` for any data affecting output or hashing
- Lazy content loading: `Artifact`/`Context` read files only when accessed
- Two-stage config: `parse_config()` (TOML deser) then `validate_config()` (semantic checks) — never merge these
- Error messages must include the offending value and enough context to act on
- All tests in `#[cfg(test)] mod tests` at bottom of each module, using `tempfile` for filesystem tests

See `docs/CONVENTIONS.md` for the full list.

## Version

**Single source of truth:** `Cargo.toml` `[package].version`. All runtime version strings use `env!("CARGO_PKG_VERSION")`. Never hardcode the version elsewhere.

## Deep References

| Topic | File |
| ----- | ---- |
| Code style, naming, formatting | `docs/STYLE.md` |
| Module layers, execution pipeline, design decisions | `docs/ARCHITECTURE.md` |
| Golden rules, mechanical conventions | `docs/CONVENTIONS.md` |
| Test patterns, running tests, what to cover | `docs/TESTING.md` |
| Anti-patterns, security, what NOT to do | `docs/BOUNDARIES.md` |
| Spec (authoritative behavior reference) | `baton-spec-v0_4.md` |
| Module-level specs (assertions, test coverage gaps) | `spec/*.md` |

## Spec Files

The `spec/` directory contains one spec file per module with machine-readable assertions (`SPEC-XX-YY-NNN`). Each assertion maps to an existing test or is marked `UNTESTED`. Use these to:

- **Find coverage gaps:** grep for `UNTESTED` to see what still needs tests
- **Understand behavior contracts:** each assertion documents a specific decision point, error return, or invariant
- **Write new tests:** the assertion IDs provide a checklist when adding or modifying functionality

Files: `types.md`, `config.md`, `prompt.md`, `placeholder.md`, `verdict_parser.md`, `exec.md`, `history.md`, `runtime.md`

## Not Yet Implemented

Timeout enforcement on script validators, signal handling (SIGINT/SIGTERM), log file writing (only SQLite history), TTY auto-detection for `--format`
