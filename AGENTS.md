# AGENTS.md

Instructions for AI coding agents working in this repository.

## What is Baton?

A composable validation gate for AI agent outputs. Accepts input files (from positional args, `--diff`, `--files`, or source declarations), runs validators (script/LLM/human) against them via a dispatch planner, produces structured results (pass/fail/error per gate), and persists invocation history in SQLite.

## Commands

```bash
cargo build                          # Build
cargo test                           # Run all tests
cargo test config::tests::test_name  # Run a single test
cargo test config::tests             # Run one module's tests
cargo clippy --all-targets -- -D warnings  # Lint (must pass with zero warnings)
cargo fmt --check                    # Formatting
```

## Development Workflow

New features and bug fixes follow spec → tests → implementation order:

1. **Edit the spec first** — add or update `SPEC-XX-YY-NNN` assertions in the relevant `spec/*.md` file. Mark new assertions as `UNTESTED`.
2. **Write tests** — implement tests that exercise the new assertions. Update the spec to reference the test name.
3. **Write implementation** — make the tests pass. The spec is the authoritative behavior reference; if the implementation disagrees with the spec, fix the implementation.

This ordering keeps the spec, tests, and code in sync. The spec drives everything — it is the single source of truth for what the system should do.

## Architecture

**Data flow:** CLI → config discovery/parse/validate → collect input files → dispatch planner → gate execution → store invocation in SQLite → output (JSON/human/summary)

**Module dependency layers** (top → bottom, never import upward):

```text
main.rs → exec, config, history, runtime, provider, types
exec → config, types, placeholder, runtime, error
runtime → types, error, provider
provider → types
config → types, placeholder, error
history, placeholder → types, error
prompt, verdict_parser → types or error only
error → (leaf, no internal imports)
```

See `docs/ARCHITECTURE.md` for the full dependency table and design rationale (two-stage config, lazy loading, Status vs VerdictStatus).

## Key Conventions

- `BTreeMap` over `HashMap` for any data affecting output or hashing
- Lazy content loading: `InputFile` reads files only when accessed via `OnceCell`
- Two-stage config: `parse_config()` (TOML deser) then `validate_config()` (semantic checks) — never merge these
- Validators are defined top-level in `[validators]`; gates reference them with optional `blocking`/`run_if` overrides
- Error messages must include the offending value and enough context to act on
- All tests in `#[cfg(test)] mod tests` at bottom of each module, using `tempfile` for filesystem tests
- Runtime adapter tests use the `session_adapter_tests!` macro in `session_common.rs` — new session adapters invoke this macro for automatic coverage

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
| Module-level specs (behavior reference, test coverage) | `spec/*.md` |
| Spec file format, assertion IDs, how to use them | `docs/SPEC.md` |

## Spec Files

The `spec/` directory contains one spec file per module. Each file is a detailed decision tree documenting every decision point, error return, and invariant as machine-readable `SPEC-XX-YY-NNN` assertions with associated tests. These are the authoritative behavior reference — see `docs/SPEC.md` for the full format guide, assertion ID conventions, and usage patterns.

Quick reference:

- **Find coverage gaps:** `grep -r "UNTESTED" spec/`
- **Understand behavior contracts:** read the prose and assertions for any function
- **Drive new features:** edit the spec first, then write tests, then implement (see Development Workflow above)
- **List spec files:** `ls spec/*.md`
- **Files:** `types.md`, `config.md`, `prompt.md`, `placeholder.md`, `verdict_parser.md`, `exec.md`, `history.md`, `runtime.md`, `provider.md`, `main.md`

## Smoke Tests

Integration tests that call real LLM runtimes. Skipped by `cargo test` (marked `#[ignore]`).

```bash
# Run with Claude Code (default — requires `claude` in PATH, authenticated)
make smoke

# Run with an API provider
BATON_SMOKE_RUNTIME=api \
BATON_SMOKE_BASE_URL=https://api.anthropic.com \
BATON_SMOKE_API_KEY_ENV=ANTHROPIC_API_KEY \
BATON_SMOKE_MODEL=claude-haiku-4-5-20251001 \
make smoke-api
```

| Variable | Default | Description |
|----------|---------|-------------|
| `BATON_SMOKE_RUNTIME` | `claude-code` | Runtime type (`claude-code`, `api`) |
| `BATON_SMOKE_BASE_URL` | `claude` | Binary path or API URL |
| `BATON_SMOKE_MODEL` | `sonnet` | Model name |
| `BATON_SMOKE_API_KEY_ENV` | *(empty)* | Env var name holding API key |
| `BATON_SMOKE_TIMEOUT` | `60` | Timeout in seconds |

In CI, smoke tests run only on `workflow_dispatch` (manual trigger) using the `api` runtime with a `ANTHROPIC_API_KEY` secret.

## Not Yet Implemented

Timeout enforcement on script validators, signal handling (SIGINT/SIGTERM), log file writing (only SQLite history), TTY auto-detection for `--format`, wiring `plan_dispatch()` into `run_gate()` loop (dispatch planner is implemented but gate execution still passes all files as a flat pool), wiring `store_invocation()` into `cmd_check()` (v2 history functions exist but `cmd_check` still calls `store_verdict()`)
