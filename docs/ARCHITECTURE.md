# Architecture

## Dependency Layers

Modules follow a strict dependency direction. Lower layers must not import from higher layers.

```
         ┌──────────┐
         │  main.rs  │  CLI entry point (clap)
         └─────┬─────┘
               │ uses
    ┌──────────┼──────────┐
    │          │          │
    ▼          ▼          ▼
  exec      config     history
    │          │
    ├──────────┤
    │          │
    ▼          ▼
placeholder  prompt
    │
    ▼
  types ◄──── verdict_parser
    │
    ▼
  error
```

**Allowed dependency direction (top → bottom):**

| Layer | May import from |
|-------|----------------|
| `main.rs` | `config`, `exec`, `history`, `types` |
| `exec` | `config`, `types`, `placeholder`, `error` |
| `config` | `types`, `placeholder`, `error` |
| `history` | `types`, `error` |
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
