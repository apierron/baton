# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is Baton?

A composable validation gate for AI agent outputs. Accepts an artifact (file to validate) + context (reference docs), runs validators (script/LLM/human), produces a structured verdict (pass/fail/error), and persists results in SQLite.

## Commands

```bash
cargo build                          # Build
cargo test                           # Run all tests (153 across 7 modules)
cargo test config::tests::test_name  # Run a single test
cargo test config::tests             # Run one module's tests
cargo clippy --all-targets -- -D warnings  # Lint (must pass with zero warnings)
```

## Architecture

**Data flow:** CLI → config discovery/parse/validate → load artifact & context → `run_gate()` → store verdict in SQLite → output (JSON/human/summary)

**Module dependency layers** (top → bottom, never import upward):
```
main.rs → exec, config, history, types
exec → config, types, placeholder, error
config → types, placeholder, error
history, placeholder → types, error
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

## Deep References

| Topic | File |
|-------|------|
| Code style, naming, formatting | `docs/STYLE.md` |
| Module layers, execution pipeline, design decisions | `docs/ARCHITECTURE.md` |
| Golden rules, mechanical conventions | `docs/CONVENTIONS.md` |
| Test patterns, running tests, what to cover | `docs/TESTING.md` |
| Anti-patterns, security, what NOT to do | `docs/BOUNDARIES.md` |
| Spec (authoritative behavior reference) | `baton-spec-v0_4.md` |

## Not Yet Implemented

LLM completion/session validators (HTTP calls stubbed), timeout enforcement, signal handling (SIGINT/SIGTERM), log file writing (only SQLite), `check-provider`/`check-runtime` commands, TTY auto-detection for `--format`