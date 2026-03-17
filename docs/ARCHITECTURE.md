# Architecture

## Dependency Layers

Modules follow a strict dependency direction. Lower layers must not import from higher layers.

```text
          ┌───────────┐
          │  main.rs  │  CLI entry point (clap)
          └─────┬─────┘
                │ uses
    ┌───────┬───┼───────┐
    │       │   │       │
    ▼       ▼   ▼       ▼
  exec   config history runtime
    │       │           │
    ├───────┤           │
    │       │     ┌─────┘
    ▼       ▼     │
placeholder prompt│
    │             │
    ▼             ▼
  types ◄──── verdict_parser
    │
    ▼
  error
```

**Allowed dependency direction (top → bottom):**

| Layer | May import from |
| ----- | --------------- |
| `main.rs` | `config`, `exec`, `history`, `runtime`, `types` |
| `exec` | `config`, `types`, `placeholder`, `runtime`, `error` |
| `config` | `types`, `placeholder`, `error` |
| `history` | `types`, `error` |
| `runtime` | `types`, `error` |
| `placeholder` | `types`, `error` |
| `prompt` | `error` |
| `verdict_parser` | `types` |
| `types` | `error` |
| `error` | *(leaf — no internal imports)* |

**Violations of this layering are bugs.** If you need to call upward, restructure: move the shared logic into the lower layer or introduce a new shared module.

## Two-Stage Config

Config parsing is intentionally split into two phases:

1. **Parse** (`RawConfig` via serde) — Deserializes TOML, performs env var substitution. No semantic validation. This stage never fails on valid TOML, even if the config is semantically wrong.
2. **Validate** (`BatonConfig`) — Checks cross-field constraints, resolves prompt file paths, validates `run_if` references. Returns structured `ValidationResult` with errors and warnings.

This separation exists so that tooling (editors, linters) can report all config errors at once rather than stopping at the first one.

## Execution Pipeline

`run_gate()` in `exec.rs` is the core orchestrator. The pipeline for a single gate:

1. Resolve which validators to run (apply `--only`, `--skip`, `--tags` filters)
2. For each validator in order:
   a. Evaluate `run_if` — skip if condition is false
   b. Resolve `{placeholders}` in command/prompt
   c. Execute (script: subprocess, LLM: HTTP, human: fail with review-requested)
   d. If `blocking=true` and validator failed → stop pipeline (unless `--all`)
   e. Record `ValidatorResult`
3. Compute `VerdictStatus` from all results (error > fail > pass)
4. Build and return `Verdict`

## Lazy Loading Pattern

`Artifact` and `Context` use lazy loading: file content and SHA-256 hash are computed on first access, not at construction time. This means:

- Creating an `Artifact::from_file()` only checks existence, not readability
- `get_content()` / `get_hash()` perform I/O and cache the result
- Placeholders like `{artifact_content}` trigger the load — if a validator doesn't reference content, the file is never read

This matters for large artifacts and for validators that only need the file path (e.g., `ruff check {artifact}`).

## Status vs VerdictStatus

Two separate enums exist intentionally:

- **`Status`** (5 values: pass, fail, warn, skip, error) — validator-level granularity. Validators can warn or skip.
- **`VerdictStatus`** (3 values: pass, fail, error) — gate-level result. Maps to exit codes 0/1/2. A gate cannot "warn" or "skip" — it must commit to a verdict.

The mapping from validator statuses to gate verdict is in `compute_final_status()`. Status suppression (`suppress_errors`, `suppress_warnings`) can override individual validator statuses before aggregation.

## LLM Validators

LLM validators operate in two modes:

- **Completion** (`exec.rs: execute_llm_completion`) — Sends a single HTTP POST to an OpenAI-compatible `/v1/chat/completions` endpoint. The prompt template is resolved with placeholders, the response is parsed by `verdict_parser` for PASS/FAIL/WARN keywords. Token counts and cost are tracked in `ValidatorResult.cost`.

- **Session** (`exec.rs: execute_llm_session`) — Delegates to a `RuntimeAdapter` (see below). Creates a multi-turn agent session, polls for completion, and collects the final result. The agent can use tools, read files, and produce a verdict grounded in observation.

`execute_validator()` takes `Option<&BatonConfig>` — `None` is fine for script/human validators, but required for LLM validators (to resolve provider/runtime configuration).

## Runtime Adapters

The `runtime` module defines the `RuntimeAdapter` trait with five methods: `create_session`, `poll_status`, `collect_result`, `cancel`, and `teardown`, plus a `health_check` for connectivity verification.

`runtime::openhands` implements this trait for the OpenHands platform. New runtime adapters (e.g., SWE-agent) should implement `RuntimeAdapter` and be wired into `runtime::create_adapter()`.

## Spec Files

Detailed behavioral specifications for each module live in `spec/*.md`. Each spec file is a complete decision tree for its module — it enumerates every decision point, error return, and invariant as machine-readable `SPEC-XX-YY-NNN` assertions, with associated tests for each assertion. These specs are the authoritative behavior reference: when the implementation disagrees with the spec, the implementation is wrong.

See `docs/SPEC.md` for the spec file format, assertion ID conventions, and the spec-driven development workflow. See `docs/TESTING.md` for how to use spec files to find coverage gaps.
