# Architecture

## Dependency Layers

Modules follow a strict dependency direction. Lower layers must not import from higher layers.

```text
          ┌───────────┐
          │  main.rs  │  CLI entry point (clap)
          └─────┬─────┘
                │ uses
    ┌───────┬───┼───────┬──────────┐
    │       │   │       │          │
    ▼       ▼   ▼       ▼          ▼
  exec   config history runtime  provider
    │       │             │          │
    ├───────┤             └──► provider
    │       │
    ▼       ▼
placeholder prompt
    │
    ▼
  types ◄──── verdict_parser
    │
    ▼
  error

Note: exec includes the dispatch planner (file collection, input matching, invocation planning) as part of its execution pipeline.
Note: exec no longer depends on provider directly; runtime (via its API adapter) handles provider interaction.
```

**Allowed dependency direction (top → bottom):**

| Layer | May import from |
| ----- | --------------- |
| `main.rs` | `config`, `exec`, `history`, `runtime`, `provider`, `types` |
| `exec` | `config`, `types`, `placeholder`, `runtime`, `error` |
| `config` | `types`, `placeholder`, `error` |
| `history` | `types`, `error` |
| `runtime` | `types`, `error`, `provider` |
| `placeholder` | `types`, `error` |
| `prompt` | `error` |
| `verdict_parser` | `types` |
| `types` | `error` |
| `provider` | `types` (for `Cost`) |
| `error` | *(leaf — no internal imports)* |

**Violations of this layering are bugs.** If you need to call upward, restructure: move the shared logic into the lower layer or introduce a new shared module.

## Two-Stage Config

Config parsing is intentionally split into two phases:

1. **Parse** (`RawConfig` via serde) — Deserializes TOML, performs env var substitution. No semantic validation. This stage never fails on valid TOML, even if the config is semantically wrong.
2. **Validate** (`BatonConfig`) — Checks cross-field constraints, resolves prompt file paths, validates `run_if` references. Returns structured `ValidationResult` with errors and warnings.

This separation exists so that tooling (editors, linters) can report all config errors at once rather than stopping at the first one.

## Execution Pipeline

Execution follows a three-stage pipeline: **collect → plan → execute**.

1. **File collector** — Gathers input files from positional args, `--diff`, `--files`, and source declarations. Directories are walked recursively. The pool is deduplicated by canonical path.
2. **Dispatch planner** — For each validator, matches the input pool against the validator's `input` declaration to produce `Invocation`s. Per-file inputs produce one invocation per matching file; batch inputs produce a single invocation with all matches; named inputs are grouped by key expression.
3. **Gate execution** — Gates are filtered by `--only` / `--skip` selectors. For each gate:
   a. Resolve which validators to run (apply selectors)
   b. For each validator's invocations in order:
      - Evaluate `run_if` — skip if condition is false
      - Resolve `{placeholders}` in command/prompt using the invocation's input files
      - Execute (script: subprocess, LLM: HTTP, human: fail with review-requested)
      - If `blocking=true` and any invocation failed → stop pipeline
      - Record `ValidatorResult` per invocation
   c. Compute `VerdictStatus` from all results (error > fail > pass)
   d. Build `GateResult`
4. Assemble `InvocationResult` from all gate results

## Lazy Loading Pattern

`InputFile` uses lazy loading: file content and SHA-256 hash are computed on first access, not at construction time. This means:

- Constructing an `InputFile` only records the path
- `get_content()` / `get_hash()` perform I/O and cache the result via `OnceCell`
- Placeholders like `{file.content}` trigger the load — if a validator doesn't reference content, the file is never read

This matters for large input files and for validators that only need the file path (e.g., `ruff check {file}`).

## Status

`Status` (5 values: pass, fail, warn, skip, error) is the single status type used at every level — validators, gates, and invocations. A gate can be skipped (filtered by `--skip`) and can produce a warning (all validators passed but some warned).

`VerdictStatus` (3 values: pass, fail, error) exists for backward compatibility with the v1 Verdict output format and CLI exit codes (0/1/2). It is a lossy reduction and should be phased out in favor of `Status` everywhere.

Status suppression (`suppress_errors`, `suppress_warnings`) can override individual validator statuses before aggregation.

## LLM Validators

LLM validators operate in two modes, both dispatched through runtimes:

- **Query** (`mode = "query"`, default) — Resolves the prompt template and placeholders, then dispatches through the appropriate runtime. For the API runtime, this delegates to `provider::ProviderClient::post_completion()`. The response content is parsed by `verdict_parser` for PASS/FAIL/WARN keywords. Token counts and cost are extracted and tracked in `ValidatorResult.cost`.

- **Session** (`mode = "session"`) — Delegates to a `RuntimeAdapter`. Creates a multi-turn agent session, polls for completion, and collects the final result. The agent can use tools, read files, and produce a verdict grounded in observation.

Validators specify their runtime via the `runtime` field (string or list) instead of the former `provider` field. `execute_validator()` takes `Option<&BatonConfig>` — `None` is fine for script/human validators, but required for LLM validators (to resolve runtime configuration).

## Provider Client

The `provider` module provides `ProviderClient`, a shared HTTP client for OpenAI-compatible LLM APIs. It handles API key resolution, Bearer auth, and structured error classification (auth failures, model-not-found, rate limiting, timeouts). It is now an internal utility used by the API adapter (`src/runtime/api.rs`) rather than being called directly from `exec.rs`. The CLI's `check-provider` command also uses it for connectivity checks.

Unlike `RuntimeAdapter` (a trait for pluggable backends), `ProviderClient` is a concrete struct — all supported LLM providers use the OpenAI-compatible API format. If a non-OpenAI-compatible provider is added, the client can be extended or a trait can be extracted at that point.

## Runtime Adapters

The `runtime` module defines the `RuntimeAdapter` trait with five methods: `create_session`, `poll_status`, `collect_result`, `cancel`, and `teardown`, plus a `health_check` for connectivity verification.

`runtime::openhands` implements this trait for the OpenHands platform. `runtime::api` implements it for direct LLM API calls (query mode). New runtime adapters (e.g., SWE-agent) should implement `RuntimeAdapter` and be wired into `runtime::create_adapter()`.

Supported runtime types: `api`, `openhands`, `opencode`.

## Spec Files

Detailed behavioral specifications for each module live in `spec/*.md`. Each spec file is a complete decision tree for its module — it enumerates every decision point, error return, and invariant as machine-readable `SPEC-XX-YY-NNN` assertions, with associated tests for each assertion. These specs are the authoritative behavior reference: when the implementation disagrees with the spec, the implementation is wrong.

See `docs/SPEC.md` for the spec file format, assertion ID conventions, and the spec-driven development workflow. See `docs/TESTING.md` for how to use spec files to find coverage gaps.
