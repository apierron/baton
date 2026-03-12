# baton.md — Spec v0.4

## A Composable Validation Gate for AI Agent Outputs

**Status:** Draft
**Date:** March 2026

---

## Table of Contents

1. [What Is Baton?](#1-what-is-baton)
2. [Core Concepts](#2-core-concepts)
3. [Context Isolation](#3-context-isolation)
4. [Configuration Format](#4-configuration-format)
5. [Prompt Templates](#5-prompt-templates)
6. [Execution Model](#6-execution-model)
7. [Runtime Hooks](#7-runtime-hooks)
8. [Reference Adapter: OpenHands](#8-reference-adapter-openhands)
9. [CLI Interface](#9-cli-interface)
10. [Library Interface (Pseudocode)](#10-library-interface-pseudocode)
11. [Verdict History and Logging](#11-verdict-history-and-logging)
12. [Integration Patterns](#12-integration-patterns)
13. [Testing Strategy](#13-testing-strategy)
14. [Appendix A: Prompt Template Library (Starters)](#appendix-a-prompt-template-library-starters)
15. [Appendix B: Deferred Features and Optimizations](#appendix-b-deferred-features-and-optimizations)

---

## 1. What Is Baton?

Baton is a configurable validation gate. It runs a user-defined sequence of checks such as deterministic scripts, LLM queries, or human reviews. The result is a structured verdict: pass or fail. An explicit validation step means automated output is only accepted if it can ***pass the Baton***.

Baton is **not** an agent orchestrator. It receives an artifact, evaluates it, and reports results. You decide what to do with the verdict. It does not conduct the orchestra, it is simply a tool for the conductor.

Baton **can** invoke LLMs for validation. When a validator's `type` is `llm`, baton can invoke either a single-shot LLM completion or a multi-turn agent session (via a runtime adapter) to perform the review. This is distinct from orchestration; see [Section 7](#7-runtime-hooks) for more details.

While Baton is primarily intended for improving QA in agentic workflows, it could also be used for reviewing human-generated content such as enforcing diverse style guides.

### Design Principles

- **User-defined.** Baton doesn't dictate what "valid" means for your domain. You describe validator steps as shell commands, LLM prompt templates, or agent settings. Community-shared validator modules are just scripts, prompt files, or agent configs.
- **Context-isolated.** Baton never receives any context other than what you provide. It does not care what produced the artifact or what will consume the verdict. This allows it to act as a stateless function, `artifact + context = verdict`. Much effort is taken to prevent context bleed as described in [Section 3](#3-context-isolation).
- **Observable.** Every step produces a structured log entry. Verdict history is persisted locally and queryable. You can answer "how often does this validator fail?" and "what was the total token cost of this gate over the last week?" without external tooling. This also allows for previous verification runs to optionally be used as context.

---

## 2. Core Concepts

### Artifact

The thing being validated. This could be code, a document, or any single file or text blob. Baton treats it as opaque input; validators interpret it.

An artifact is specified as a single file path or piped from stdin.

**Edge cases:**

- If the artifact path does not exist, baton exits with an error before running any validators.
- If the artifact path is a directory, baton exits with an error. Callers that want to validate a directory should archive it or concatenate its contents into a single file.
- If `--artifact -` is passed, baton reads stdin to EOF, writes the content to a temporary file (in `.baton/tmp/`), and uses that file as the artifact. The temp file is cleaned up after the gate completes (see also [Section 6.7](#67--signal-handling-and-cleanup)).
- If the artifact is empty (zero bytes), baton proceeds normally; an empty file is a valid artifact. Validators are responsible for handling empty input.

### Context

Optional reference material provided alongside the artifact. This might include the original task description, a specification, expected outputs, schemas, or any grounding documents the validators need.

Context is **explicitly passed by the caller**, never inferred or accumulated. Multiple pieces of context can be passed at once. Each context item has a name (used to reference it in validator configs) and a source (a file path or an inline string).

**Edge cases:**

- If a context path does not exist, baton exits with an error before running any validators.
- If a context path is a directory, baton exits with an error.
- If a gate declares a context slot as `required = true` and the caller does not provide it, baton exits with an error.
- If the caller provides a context item not declared in the gate's context schema, baton emits a warning to stderr and ignores it. This prevents silent misconfiguration (typos in context names).
- Context items are hashed (SHA-256) alongside the artifact hash in the verdict. This means that validating the same artifact with different context produces distinguishable history entries.

### Validator

A single check that examines the artifact (and optionally context) and returns a result. Three types are defined in this spec:

- **`script`** — Runs a shell command. Exit code 0 = pass, nonzero = fail. An optional `warn_exit_codes` list maps specific exit codes to `warn` instead of `fail` (see [Section 4.2](#42--configuration-field-reference)). Stdout/stderr is captured as feedback.
- **`llm`** — Invokes a language model for validation. Operates in one of two modes:
  - **`completion`** (default) — Sends the artifact and a prompt template to a model. The response is parsed for a structured verdict. This is a single request/response.
  - **`session`** — Launches a multi-turn agent session (via a runtime adapter) with a task prompt. The agent can use tools, read files, execute code, and produce a verdict grounded in observation rather than static analysis. See [Section 7](#7-runtime-hooks).
- **`human`** — Halts the pipeline and reports a failure with a human-review prompt as feedback. The caller is responsible for collecting human input and deciding whether to re-run the gate. Baton does not block waiting for human input — it simply fails with a clear signal that human review was requested.

### Gate

An ordered sequence of validators that together constitute the validation pipeline for a given artifact type. Gates are defined in a `baton.toml` config file. A project may define multiple named gates for different artifact types or pipeline stages.

### Verdict

The output of a gate. A structured object containing the validation outcome, feedback, and metadata.

**Verdict schema:**

```
Verdict:
  status:         "pass" | "fail" | "error"
  gate:           string                    # name of the gate that was run
  failed_at:      string | null             # validator name, if fail/error
  feedback:       string | null             # from the failing validator
  duration_ms:    integer                   # total wall-clock time for the gate
  timestamp:      ISO 8601 datetime         # when the gate completed
  artifact_hash:  string                    # SHA-256 of the input artifact
  context_hash:   string                    # SHA-256 of sorted, concatenated context hashes
  warnings:       list[string]              # validator names that returned warn
  suppressed:     list[string]              # statuses that were suppressed (e.g., ["error"])
  history:                                  # results from each validator that ran
    - name:        string
      status:      "pass" | "fail" | "warn" | "skip" | "error"
      feedback:    string | null
      duration_ms: integer
      cost:        Cost | null              # token cost, if applicable
```

**Status semantics:**

| Status | Meaning |
|--------|---------|
| `pass` | All validators passed (warnings are allowed). |
| `fail` | At least one validator returned `fail`. The artifact did not meet a validation criterion. |
| `error` | At least one validator encountered an error (crash, unreachable API, unparseable response). Distinct from `fail`: the validator could not produce a judgment. |

If both `fail` and `error` occur in the same run (possible with `--all`), the verdict status is `error`. The rationale: an error means the validation is incomplete, so even a co-occurring failure cannot be trusted as the full picture. This precedence rule is overridden when status suppression is active; see [Section 9](#9-cli-interface) for `--suppress-errors`, `--suppress-warnings`, and `--suppress-all`.

**Individual validator statuses:**

| Status | Meaning |
|--------|---------|
| `pass` | The validator ran and the artifact met its criteria. |
| `fail` | The validator ran and the artifact did not meet its criteria. |
| `warn` | The validator ran and flagged a non-blocking concern. Does not cause gate failure. |
| `skip` | The validator was skipped (due to `run_if`, `--only`, `--skip`, or `--tags` filtering). |
| `error` | The validator could not execute (crash, timeout, API failure, unparseable output). |

**Cost schema (for LLM validators):**

```
Cost:
  input_tokens:  integer | null
  output_tokens: integer | null
  model:         string | null
  estimated_usd: float | null
```

Implementations must always populate `input_tokens` and `output_tokens` when the underlying API returns them. `estimated_usd` is optional and may be omitted if pricing data is not available. Even if cost estimation is not implemented initially, the schema must be present and the token fields populated — this ensures that cost control features can be layered on later without schema changes.

**Exit code mapping (for CLI use):**

| Status | Exit code |
|--------|-----------|
| pass   | 0         |
| fail   | 1         |
| error  | 2         |

The JSON verdict is written to stdout. Human-readable summaries go to stderr.

---

## 3. Context Isolation

This is perhaps baton's most opinionated design choice and deserves explicit discussion.

### The Problem

When an LLM agent produces code and an LLM validator reviews it, there is a risk of **adversarial context bleed**: if the validator can see the producing agent's reasoning, conversation history, or self-assessment, the review is compromised. The validator may defer to the producer's confidence, inherit its blind spots, or (in adversarial setups) the producer may embed reasoning that strategically misleads the validator.

Even without adversarial intent, shared context creates **correlated failures**. If both agents see the same misleading task description, both may make the same misinterpretation. Independent validation requires independent context.

### The Rule

Baton validators receive ONLY:

1. The **artifact** (the thing being validated)
2. The **context items** explicitly listed in `context_refs`
3. The **prompt template**

They never receive the producing agent's conversation log, system prompt, internal reasoning, tool call history, or any metadata about how the artifact was produced. The baton API simply doesn't accept those fields.

For `llm` validators with `mode = "session"`, this isolation is enforced structurally: each agent session is a fresh runtime invocation with no shared state from the producing agent.

### Implications

This means an LLM validator cannot ask clarifying questions of the producing agent. If the validator needs more context, that context must be provided as a reference document. This is a feature, not a limitation: it forces the caller to make the specification explicit enough that an independent reviewer can evaluate against it.

Validators within a single gate run can see limited information about prior validators via `{verdict.<n>.*}` placeholders. This is opt-in and explicit — a validator must specifically reference another validator's output in its prompt template. The information flows only forward (no backward references) and is limited to status and feedback text.

---

## 4. Configuration Format

Baton uses TOML for configuration. One file can define multiple named gates, shared defaults, provider configuration, and runtime settings.

### 4.1 — Full Configuration Reference

```toml
# baton.toml

version = "0.4"

# ─── Defaults ──────────────────────────────────────────────
[defaults]
timeout_seconds = 300            # per-validator default
blocking = true                  # per-validator default (can be overridden per validator)
prompts_dir = "./prompts"        # where to find prompt template files
log_dir = "./.baton/logs"        # where verdict logs are written
history_db = "./.baton/history.db"  # SQLite verdict history
tmp_dir = "./.baton/tmp"         # temporary files (stdin artifacts, session files)

# ─── Provider Configuration ────────────────────────────────
# Providers map logical names to concrete API endpoints.
# Validators reference providers by key. If a validator does not
# specify a provider, the default provider is used.
#
# Any provider that exposes an OpenAI-compatible /v1/chat/completions
# endpoint works. This includes OpenAI, Anthropic (via proxy),
# Ollama, vLLM, llama.cpp, LM Studio, and similar local servers.
# Use `baton check-provider <key>` to verify connectivity and
# model availability.
#
# Design note: cross-lineage validation (e.g., producing agent uses
# Claude, validator uses GPT, or vice versa) is recommended where
# budget allows. Decorrelated model lineages reduce the probability
# of shared blind spots. This is not enforced, but a good default
# when selecting provider/model pairs for LLM validators.

[providers.default]
api_base = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"    # env var name, not the key itself
default_model = "claude-haiku"

[providers.openai]
api_base = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
default_model = "gpt-4o-mini"

# Provider for a local/self-hosted model (Ollama, vLLM, llama.cpp, etc.)
[providers.local]
api_base = "http://localhost:11434/v1"
api_key_env = ""                     # empty string = no auth required
default_model = "llama3"

# ─── Agent Runtimes ────────────────────────────────────────
# Runtimes are used by `llm` validators with `mode = "session"`.
# See Section 7 for the hook interface and Section 8 for the
# OpenHands reference adapter.

[runtimes.openhands]
type = "openhands"
base_url = "http://localhost:3000"
api_key_env = "OPENHANDS_API_KEY"    # optional, depends on deployment
default_model = "claude-sonnet"      # model the agent itself uses
sandbox = true                       # run in sandboxed container
timeout_seconds = 600                # agent sessions can be long
max_iterations = 30                  # cap agent tool-use loops

# ─── Environment Variable Interpolation ────────────────────
# Any string value in this file can reference environment variables
# using the syntax ${VAR_NAME} or ${VAR_NAME:-default_value}.
# Example: api_base = "${CUSTOM_API_BASE:-https://api.anthropic.com}"
#
# Edge cases:
# - If ${VAR_NAME} references an unset variable with no default,
#   baton exits with an error during config loading.
# - If ${VAR_NAME:-} is used (empty default), the value resolves
#   to an empty string.
# - Interpolation is not recursive: ${${OTHER}} is not supported.
# - Literal "${" can be escaped as "$${" (resolves to "${").

# ─── Gates ─────────────────────────────────────────────────
[gates.code-review]
description = "Validates a code patch against a task spec"

  # What reference material validators can access
  [gates.code-review.context.spec]
  description = "The task specification this code should satisfy"
  required = true

  [gates.code-review.context.prior_output]
  description = "Output from the previous pipeline stage"
  required = false

  # ── Tier 1: Deterministic / fast / free ──

  [[gates.code-review.validators]]
  name = "lint"
  type = "script"
  command = "ruff check {artifact}"
  warn_exit_codes = [2]            # exit 2 = warnings only (pass with feedback)
  blocking = true

  [[gates.code-review.validators]]
  name = "typecheck"
  type = "script"
  command = "mypy --strict {artifact}"
  blocking = true

  [[gates.code-review.validators]]
  name = "tests"
  type = "script"
  command = "pytest {artifact_dir}/tests/ -x --tb=short"
  blocking = true

  # ── Tier 2: Cheap LLM / structured check ──

  [[gates.code-review.validators]]
  name = "spec-compliance"
  type = "llm"
  mode = "completion"                # default; explicit here for clarity
  provider = "default"
  model = "claude-haiku"
  prompt = "spec-compliance.md"      # resolves to ./prompts/spec-compliance.md
  context_refs = ["spec"]
  temperature = 0.0
  response_format = "verdict"
  blocking = true
  max_tokens = 4096

  # ── Tier 3: Deep review (agent session, conditional) ──

  [[gates.code-review.validators]]
  name = "adversarial-review"
  type = "llm"
  mode = "session"
  runtime = "openhands"
  prompt = "adversarial-review.md"   # resolves to ./prompts/adversarial-review.md
  context_refs = ["spec", "prior_output"]
  run_if = "spec-compliance.status == pass"
  blocking = true

  # ── Tier 4: Human gate (optional) ──

  [[gates.code-review.validators]]
  name = "human-review"
  type = "human"
  prompt = "Adversarial review passed but this gate requires human sign-off."
  run_if = "adversarial-review.status == pass"
  blocking = true


# ─── A simpler gate for non-code artifacts ─────────────────
[gates.doc-review]
description = "Validates a document or plan"

  [[gates.doc-review.validators]]
  name = "schema-check"
  type = "script"
  command = "ajv validate -s {context.schema} -d {artifact}"
  blocking = true

  [[gates.doc-review.validators]]
  name = "completeness-check"
  type = "llm"
  model = "claude-haiku"
  prompt = "doc-completeness.md"
  context_refs = ["spec"]
  response_format = "verdict"
  blocking = true
```

### 4.2 — Configuration Field Reference

**Top-level fields:**

| Field | Type | Required | Description |
|---|---|---|---|
| `version` | string | yes | Spec version. Must be `"0.4"` for this spec. |
| `defaults` | table | no | Default values for timeouts, paths, and validator behavior. |
| `providers` | table | no | Named LLM provider configurations. |
| `runtimes` | table | no | Named agent runtime configurations. |
| `gates` | table | yes | Named validation gates. At least one gate must be defined. |

**Defaults fields:**

| Field | Type | Default | Description |
|---|---|---|---|
| `timeout_seconds` | integer | 300 | Per-validator timeout. |
| `blocking` | boolean | `true` | Per-validator default for `blocking`. Validators can override. |
| `prompts_dir` | string | `"./prompts"` | Directory for prompt template files. |
| `log_dir` | string | `"./.baton/logs"` | Directory for structured log files. |
| `history_db` | string | `"./.baton/history.db"` | Path to SQLite verdict history database. |
| `tmp_dir` | string | `"./.baton/tmp"` | Directory for temporary files. |

**Provider fields:**

| Field | Type | Required | Description |
|---|---|---|---|
| `api_base` | string | yes | Base URL for the API. |
| `api_key_env` | string | yes | Name of environment variable holding the API key. Empty string = no auth. |
| `default_model` | string | yes | Default model identifier for this provider. |

**Edge cases for providers:**

- If `api_key_env` names a variable that is not set, baton reports an error at config load time (not at validator execution time). Fail early.
- If `api_key_env` is empty string, no `Authorization` header is sent. This is the expected configuration for local model servers that do not require auth.
- `api_base` must not have a trailing slash. Baton strips a trailing slash if present during config normalization.

**Runtime fields (for `llm` validators with `mode = "session"`):**

| Field | Type | Required | Description |
|---|---|---|---|
| `type` | string | yes | Runtime type identifier. Determines which adapter to load. See [Section 8](#8-reference-adapter-openhands) for `"openhands"`. |
| `base_url` | string | yes | URL of the agent runtime API. |
| `api_key_env` | string | no | Environment variable for runtime auth. |
| `default_model` | string | no | Model the agent uses internally. |
| `sandbox` | boolean | no | Whether to run in a sandboxed environment. Default: `true`. |
| `timeout_seconds` | integer | no | Session timeout. Default: 600. |
| `max_iterations` | integer | no | Max tool-use iterations per session. Default: 30. |

**Gate fields:**

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | string | no | Human-readable description of the gate's purpose. |
| `context` | table | no | Named context slots with descriptions and required flags. |
| `validators` | array of tables | yes | Ordered list of validators. At least one validator must be defined. |

**Validator fields (common to all types):**

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Unique identifier within the gate. Must match `[A-Za-z0-9_-]+`. |
| `type` | enum | yes | `"script"`, `"llm"`, or `"human"`. |
| `blocking` | boolean | no | Whether failure/error halts the pipeline. Falls back to `[defaults].blocking` (default: `true`). |
| `run_if` | string | no | Condition referencing prior validator results. |
| `timeout_seconds` | integer | no | Override default timeout for this validator. |
| `tags` | array of strings | no | Tags for filtering validators via CLI (e.g., `["deep", "expensive"]`). |

**Additional fields for `script` validators:**

| Field | Type | Required | Description |
|---|---|---|---|
| `command` | string | yes | Shell command. Supports placeholders: `{artifact}`, `{artifact_dir}`, `{context.<n>}`. |
| `warn_exit_codes` | array of integers | no | Exit codes that produce `warn` instead of `fail`. Default: `[]` (empty — all nonzero exit codes are `fail`). |
| `working_dir` | string | no | Working directory for the command. Default: the directory containing the artifact. |
| `env` | table | no | Additional environment variables to set for the command. |

**Script exit code mapping:**

Exit code 0 always means `pass`. Exit codes listed in `warn_exit_codes` produce `warn` with stdout/stderr captured as feedback. All other nonzero exit codes produce `fail`. Note that `warn` never halts the pipeline regardless of the `blocking` setting — a blocking script validator will halt on `fail` exit codes but not on `warn` exit codes. This is consistent with how `warn` is treated across all validator types.

**Additional fields for `llm` validators:**

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `mode` | enum | no | `"completion"` | `"completion"` for single-shot or `"session"` for multi-turn agent. |
| `provider` | string | no | `"default"` | Provider key from `[providers]`. |
| `model` | string | no | provider default | Model identifier. Overrides provider default. |
| `prompt` | string | yes | — | A filename with extension (resolved from `prompts_dir` or as a path) or an inline prompt string. See [Section 5](#5-prompt-templates). |
| `context_refs` | array | no | `[]` | Which context items to include in the prompt. |
| `temperature` | float | no | `0.0` | Model temperature. |
| `response_format` | enum | no | `"verdict"` | `"verdict"` (structured PASS/FAIL/WARN) or `"freeform"` (raw text). |
| `max_tokens` | integer | no | — | Max tokens for the response. Falls back to provider/model default. |
| `system_prompt` | string | no | — | Override the system prompt. Default: baton provides a minimal system prompt instructing structured output. |

**Note on `freeform` response format:** Freeform validators always return `warn` status with the full LLM response as feedback. This means `blocking = true` has no effect on freeform validators — `warn` never halts the pipeline. If `blocking = true` is set on a freeform validator, `baton validate-config` emits a warning: `"Validator '{name}': blocking has no effect with response_format 'freeform' (freeform always returns warn)."`.

**Additional fields for `llm` validators with `mode = "session"`:**

| Field | Type | Required | Description |
|---|---|---|---|
| `runtime` | string | yes | Runtime key from `[runtimes]`. Required when `mode = "session"`. |
| `sandbox` | boolean | no | Override the runtime's sandbox setting. |
| `max_iterations` | integer | no | Override the runtime's max iterations. |

If `mode = "session"` is set without a `runtime` field, baton reports a config validation error.

If `mode = "completion"` is set (or defaulted) and a `runtime` field is present, baton reports a config validation warning — the runtime field is ignored in completion mode.

**Additional fields for `human` validators:**

| Field | Type | Required | Description |
|---|---|---|---|
| `prompt` | string | yes | Message included in the failure feedback. Supports placeholders. |

When a `human` validator executes, it immediately returns `fail` with the rendered prompt as feedback. The calling system is responsible for presenting this to a human, collecting their decision, and optionally re-running the gate. This keeps baton non-blocking and stateless.

### 4.3 — Placeholder Reference

Placeholders are substituted at runtime in `command`, `prompt`, and `system_prompt` fields.

| Placeholder | Resolves to |
|---|---|
| `{artifact}` | Absolute path to the artifact file. |
| `{artifact_dir}` | Absolute path to the artifact's parent directory. |
| `{artifact_content}` | Inline content of the artifact (for LLM prompts). |
| `{context.<n>}` | Absolute path to the named context item (in `command` fields). |
| `{context.<n>.content}` | Inline content of the named context item (in `prompt` fields). |
| `{verdict.<validator_name>.status}` | Status of a prior validator (for use in prompts). |
| `{verdict.<validator_name>.feedback}` | Feedback from a prior validator (for use in prompts). |

**Edge cases for placeholder resolution:**

- If a placeholder references a context item not provided by the caller, and that context item is not `required`, the placeholder resolves to the empty string and a warning is emitted to stderr.
- If a placeholder references a context item not provided by the caller, and that context item IS `required`, this should have already been caught before any validators run. If it somehow reaches placeholder resolution, it is an error.
- If a placeholder references a validator name that does not exist in the gate, baton reports a config validation error (caught by `baton validate-config`).
- If `{verdict.<n>.status}` references a validator that was skipped, it resolves to `"skip"`.
- If `{verdict.<n>.status}` references a validator that has not yet run (appears later in the pipeline), baton reports a config validation error. Forward references are not allowed.
- If `{artifact_content}` is used for a very large file, it is included verbatim. Baton does not truncate. If this exceeds the model's context window, the provider will return an error, which baton surfaces as a validator `error`.
- Unrecognized placeholders (e.g., `{typo}`) are left as literal strings and a warning is emitted to stderr during execution.

### 4.4 — Prompt Resolution

The `prompt` field on `llm` validators uses the following resolution order:

1. **If the value contains a file extension** (`.md`, `.txt`, `.prompt`, `.j2`): treat as a file reference.
   - First, check `<prompts_dir>/<value>`. If it exists, load that file.
   - Second, check `<value>` as a literal path (absolute or relative to the config file's directory). If it exists, load that file.
   - If neither exists, report an error.
2. **Otherwise:** treat as an inline prompt string.

This means simple validators can define prompts inline in the TOML, while complex validators can reference standalone prompt files. Both support the same placeholder syntax.

**Edge cases:**

- A prompt value of `"spec-compliance"` (no extension) is treated as inline text. To reference a file, use `"spec-compliance.md"`.
- Inline prompts ending in a file extension are a known and accepted shortcoming of this approach.
- A prompt value of `"./custom/check.prompt"` with a recognized extension is treated as a file reference.
- Inline prompts in TOML can use multi-line strings: `prompt = """..."""`.
- If a file reference resolves to a file that is not valid UTF-8, baton reports an error.
- If a file reference resolves to an empty file, baton reports an error (an empty prompt is never intentional).

---

## 5. Prompt Templates

LLM validators can reference prompt templates stored as files. A prompt template is a text file (typically markdown) with optional TOML frontmatter delimited by `+++`:

```markdown
+++
description = "Check whether an implementation satisfies its specification"
expects = "verdict"
+++

You are a code reviewer. You will be given a specification and an implementation.

Your job is to determine whether the implementation satisfies the specification.
Do NOT look for style issues, performance improvements, or alternative approaches.
ONLY check whether the spec requirements are met.

Respond with exactly one of:
- PASS — if all spec requirements are satisfied
- FAIL — if any spec requirement is not satisfied

If FAIL, cite the specific requirement that is not met and explain why.

If the implementation fully satisfies the spec, you MUST respond PASS.
Do not invent requirements that are not in the spec.

## Specification

{context.spec.content}

## Implementation

{artifact_content}
```

### Template Frontmatter Fields

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | string | no | Human-readable description of what this template checks. |
| `expects` | enum | yes | `"verdict"` or `"freeform"`. Must match the validator's `response_format`. |

The template identity is derived from its filename (without extension). There is no separate `id` field — the filename is the canonical identifier. This avoids inconsistencies between filename and metadata.

**Filename rules:**

- Filenames must match `[A-Za-z0-9_-]+` (before the extension). This is the subset of characters that is safe across all major operating systems and filesystems.
- If a filename contains characters outside this set, `baton validate-config` reports a warning.

### Frontmatter Parsing

```
function parse_template(file_path: string) → PromptTemplate:
  raw = read_file(file_path)

  if raw starts with "+++":
    # Find closing "+++"
    end_index = raw.index_of("+++", start=3)
    if end_index == NOT_FOUND:
      error("Template {file_path}: opening +++ without closing +++")

    frontmatter_text = raw[3:end_index].strip()
    body = raw[end_index + 3:].strip()

    frontmatter = parse_toml(frontmatter_text)
    # Validate 'expects' field
    if "expects" not in frontmatter:
      error("Template {file_path}: frontmatter missing required 'expects' field")
    if frontmatter["expects"] not in ["verdict", "freeform"]:
      error("Template {file_path}: 'expects' must be 'verdict' or 'freeform', got '{frontmatter.expects}'")
  else:
    # No frontmatter. Body is the entire file.
    frontmatter = {expects: "verdict"}   # default
    body = raw.strip()

  if body is empty:
    error("Template {file_path}: prompt body is empty")

  return PromptTemplate(
    name = filename_without_extension(file_path),
    description = frontmatter.get("description", null),
    expects = frontmatter["expects"],
    body = body
  )
```

### Template Design Guidelines

These should be refined by empirical testing, but initial principles:

- **Ground the validator.** Always provide a reference document (spec, schema, requirements) and instruct the LLM to evaluate against it, not against its own judgment.
- **Make PASS a first-class outcome.** Constrain the output so that `PASS` is always valid. If the validator can't say "this is fine," it will hallucinate objections.
- **Scope the review.** Explicitly exclude things the validator should NOT check. An LLM told to "find problems" will always find problems.
- **Separate concerns.** One prompt template per validation concern. Don't ask a single prompt to check correctness, style, performance, and security simultaneously.
- **Demand evidence.** For FAIL verdicts, require a specific citation (spec section, failing input, reproduction case). Speculation is not sufficient.
- **Include WARN guidance.** If you want validators to use WARN, instruct them explicitly. An LLM will default to PASS or FAIL unless told that WARN is an option and when to use it.

---

## 6. Execution Model

### 6.1 — Pipeline Execution

Validators run in the order defined. When a validator with `blocking = true` fails or errors, the pipeline halts and returns the verdict immediately. Non-blocking validators (`blocking = false`) record their result but allow the pipeline to continue. **Non-blocking failures are recorded in the verdict history but do not affect the gate's final status in normal (non-`--all`) mode.** A gate with only passing and non-blocking-failing validators produces a `pass` verdict.

```
artifact
  │
  ▼
┌────────────--─┐   fail + blocking   ┌─────────┐
│  validator 1  │ ──────────────────► │ VERDICT │
│  (lint)       │                     │  fail   │
└──────┬───────-┘                     └─────────┘
       │ pass
       ▼
┌────────────-──┐   fail + blocking   ┌─────────┐
│  validator 2  │ ──────────────────► │ VERDICT │
│  (typecheck)  │                     │  fail   │
└──────┬──────-─┘                     └─────────┘
       │ pass
       ▼
┌─────────────-─┐   warn              ┌──────────────┐
│  validator 3  │ ──────────────────► │  (recorded)  │
│  (style hint) │                     │  continues   │
└──────┬───────-┘                     └──────┬───────┘
       │                                     │
       ▼                                     ▼
┌──────────────-┐   fail + blocking   ┌─────────┐
│  validator 4  │ ──────────────────► │ VERDICT │
│  (llm review) │                     │  fail   │
└──────┬───────-┘                     └─────────┘
       │ pass
       ▼
   ┌──────────┐
   │ VERDICT  │
   │  pass    │
   │ (w/warns)│
   └──────────┘
```

**The `--all` flag** changes two behaviors simultaneously:

1. **Blocking is ignored.** Every validator runs regardless of prior failures.
2. **Non-blocking failures count toward the verdict.** Unlike normal mode where only blocking failures determine the verdict, `--all` mode considers every validator's status when computing the final result. This means a non-blocking failure that would be invisible in normal mode produces a `fail` verdict in `--all` mode.

This dual behavior is intentional: `--all` mode is designed to give a complete picture of all validation problems, which requires both running everything and counting everything.

**Determining the gate verdict when `--all` is set:**

```
function compute_final_status(
  results: list[ValidatorResult],
  suppressed: set[string]           # statuses to suppress (e.g., {"error"})
) → string:
  statuses = [r.status for r in results if r.status != "skip"]

  # Apply suppression: treat suppressed statuses as "pass"
  effective_statuses = []
  for s in statuses:
    if s in suppressed:
      effective_statuses.append("pass")
    else:
      effective_statuses.append(s)

  if "error" in effective_statuses:
    return "error"
  if "fail" in effective_statuses:
    return "fail"
  # "warn" and "pass" both result in a passing gate
  return "pass"
```

**Determining the gate verdict's `failed_at` and `feedback` fields when `--all` is set:**

When `--all` mode produces a `fail` or `error` verdict, the top-level `failed_at` and `feedback` fields report only the first validator matching the final status. The full picture is always available in the `history` array. Callers that need to display all failures should iterate over `history` rather than relying on the top-level fields.

### 6.2 — Conditional Execution

The `run_if` field allows validators to run only when certain conditions are met. This enables tiered escalation:

```toml
[[gates.code-review.validators]]
name = "deep-review"
run_if = "quick-review.status == pass"
# Only spend tokens on deep review if quick review passed —
# no point in deep-reviewing code that already failed basic checks
```

Expression syntax is deliberately minimal:

```
run_if = "<validator_name>.status == <pass|fail|warn|error|skip>"
run_if = "<expr> and <expr>"
run_if = "<expr> or <expr>"
```

When a validator is skipped due to `run_if` evaluating to false, its status is recorded as `"skip"` in the verdict history.

**Operator precedence:** There is no operator precedence. Expressions are evaluated strictly left-to-right. `"a or b and c"` is evaluated as `"(a or b) and c"`, not `"a or (b and c)"`. This is a deliberate simplification; complex conditions should be decomposed into multiple validators with simpler `run_if` expressions.

**Short-circuit evaluation:** There is no short-circuit evaluation. All atoms in the expression are always evaluated, even when the result is already determined. This ensures that invalid expressions (referencing nonexistent validators, syntax errors) are caught regardless of the values of earlier atoms.

**Edge case — filtered dependencies:** If a `run_if` expression references a validator that was excluded by `--only`, `--skip`, or `--tags` filtering, the referenced validator's status is `"skip"`. This means the `run_if` condition will evaluate against `"skip"` — which typically causes the dependent validator to also be skipped. Baton emits a warning to stderr when this happens (e.g., `"warning: 'deep-review' depends on 'quick-review' which was filtered out"`).

```
function evaluate_run_if(expr: string, prior_results: map[string → ValidatorResult]) → boolean:
  # Tokenize: split on " and " / " or " (space-delimited to avoid matching substrings)
  # Each atom is: "<n>.status == <value>"

  tokens = split_on_operators(expr)
  # Returns a list alternating between atoms and operators:
  # e.g., ["lint.status == pass", "and", "typecheck.status == pass"]

  # Evaluate first atom
  current_result = evaluate_atom(tokens[0], prior_results)

  # Evaluate remaining atoms left-to-right, no precedence, no short-circuit.
  # All atoms are always evaluated so that invalid expressions are caught
  # even when early results might determine the outcome.
  i = 1
  while i < len(tokens):
    operator = tokens[i]       # "and" or "or"
    next_result = evaluate_atom(tokens[i + 1], prior_results)

    if operator == "and":
      current_result = current_result and next_result
    else if operator == "or":
      current_result = current_result or next_result
    else:
      error("Invalid operator in run_if: '{operator}'. Expected 'and' or 'or'.")

    i += 2

  return current_result


function evaluate_atom(atom: string, prior_results: map[string → ValidatorResult]) → boolean:
  parts = atom.split(".status == ")
  if len(parts) != 2:
    error("Invalid run_if expression: '{atom}'. Expected '<n>.status == <value>'")

  validator_name = parts[0].strip()
  expected_status = parts[1].strip()

  if expected_status not in ["pass", "fail", "warn", "error", "skip"]:
    error("Invalid status in run_if: '{expected_status}'")

  if validator_name not in prior_results:
    # Validator hasn't run yet and isn't in results — treat as skip
    return (expected_status == "skip")
  else:
    return (prior_results[validator_name].status == expected_status)
```

### 6.3 — Timeout Handling

Each validator has a timeout (from its own `timeout_seconds` field, or from `[defaults].timeout_seconds`). If a validator exceeds its timeout:

1. The process/request is terminated.
   - For `script` validators: send SIGTERM to the process group. If the process has not exited after 5 seconds, send SIGKILL.
   - For `llm` validators (completion mode): cancel the HTTP request.
   - For `llm` validators (session mode): call the runtime's `cancel` hook.
2. The validator's status is recorded as `"error"` (not `"fail"` — the validator did not produce a judgment).
3. The feedback is set to `"[baton] Validator timed out after {n} seconds"`.
4. Pipeline execution continues according to the `blocking` setting.

### 6.4 — Error Handling

If a validator encounters an error (as distinct from returning a fail verdict), its status is `"error"`. Errors mean the validator could not produce a judgment about the artifact. The feedback includes diagnostic information.

Baton does not automatically retry on errors, including rate limits (HTTP 429). The `Retry-After` value is captured in the feedback message for the caller's benefit, but baton itself treats 429 as an immediate error. Callers who want retry behavior should implement it in their orchestration layer.

**Exhaustive error conditions by validator type:**

**`script` validators:**

| Condition | Feedback message |
|---|---|
| Command not found | `"[baton] Command not found: {command}"` |
| Permission denied | `"[baton] Permission denied: {command}"` |
| Timeout | `"[baton] Validator timed out after {n} seconds"` |
| Killed by signal | `"[baton] Process killed by signal {signal}"` |
| Working directory does not exist | `"[baton] Working directory not found: {path}"` |

Note: a non-zero exit code from a script validator is a `fail` (or `warn` if the exit code is listed in `warn_exit_codes`), not an `error`. The script ran and returned a result — the result was negative. The conditions above are cases where the script could not run at all.

**`llm` validators (completion mode):**

| Condition | Feedback message |
|---|---|
| Provider unreachable | `"[baton] Cannot reach provider '{provider}' at {api_base}: {error}"` |
| Authentication failure (401/403) | `"[baton] Authentication failed for provider '{provider}'. Check {api_key_env}."` |
| Model not found (404) | `"[baton] Model '{model}' not found on provider '{provider}'."` |
| Rate limited (429) | `"[baton] Rate limited by provider '{provider}'. Retry-After: {n}s"` |
| Context length exceeded | `"[baton] Input exceeds context window for model '{model}' ({n} tokens). Reduce artifact/context size."` |
| Malformed response (no content) | `"[baton] Provider returned empty or malformed response."` |
| Verdict parse failure | `"[baton] Could not parse verdict from validator output:\n{raw_output}"` |
| Timeout | `"[baton] Validator timed out after {n} seconds"` |
| Unexpected HTTP status | `"[baton] Provider returned HTTP {status}: {body}"` |

**`llm` validators (session mode):**

| Condition | Feedback message |
|---|---|
| Runtime unreachable | `"[baton] Cannot reach runtime '{runtime}' at {base_url}: {error}"` |
| Session creation failed | `"[baton] Failed to create session on runtime '{runtime}': {error}"` |
| Iteration limit reached | `"[baton] Agent hit iteration limit ({n}) without producing a verdict."` |
| Sandbox creation failed | `"[baton] Sandbox setup failed on runtime '{runtime}': {error}"` |
| Agent produced no verdict | `"[baton] Agent session completed but output contained no PASS/FAIL/WARN verdict."` |
| Timeout | `"[baton] Agent session timed out after {n} seconds"` |

**`human` validators:**

Human validators always return `fail` (with the prompt as feedback). They do not produce errors under normal operation. If the prompt template fails to render (bad placeholder), that is an error.

### 6.5 — Validator Execution Pseudocode (Consolidated)

This pseudocode shows the full execution of a single validator, including error handling:

```
function execute_validator(
  validator: ValidatorConfig,
  artifact: Artifact,
  context: Context,
  prior_results: map[string → ValidatorResult]
) → ValidatorResult:

  start_time = now()

  try:
    # ── Evaluate run_if ──
    if validator.run_if is not null:
      should_run = evaluate_run_if(validator.run_if, prior_results)
      if not should_run:
        return ValidatorResult(
          name = validator.name,
          status = "skip",
          feedback = null,
          duration_ms = 0,
          cost = null
        )

    # ── Dispatch by type ──
    if validator.type == "script":
      result = execute_script_validator(validator, artifact, context)

    else if validator.type == "llm" and validator.mode == "completion":
      result = execute_llm_completion_validator(validator, artifact, context, prior_results)

    else if validator.type == "llm" and validator.mode == "session":
      result = execute_llm_session_validator(validator, artifact, context, prior_results)

    else if validator.type == "human":
      result = execute_human_validator(validator, artifact, context, prior_results)

    else:
      error("Unknown validator type: {validator.type}")

    result.duration_ms = elapsed_ms(start_time)
    return result

  catch TimeoutError:
    return ValidatorResult(
      name = validator.name,
      status = "error",
      feedback = "[baton] Validator timed out after {validator.timeout_seconds} seconds",
      duration_ms = elapsed_ms(start_time),
      cost = null
    )

  catch Exception as e:
    return ValidatorResult(
      name = validator.name,
      status = "error",
      feedback = "[baton] Unexpected error: {e}",
      duration_ms = elapsed_ms(start_time),
      cost = null
    )
```

### 6.6 — Full Gate Run Pseudocode

```
method Gate.run(artifact: Artifact, context: Context, options: RunOptions) → Verdict:
  # ── Pre-flight checks ──

  # 1. Validate artifact
  if artifact.path is not null and not file_exists(artifact.path):
    exit_error("Artifact not found: {artifact.path}")
  if artifact.path is not null and is_directory(artifact.path):
    exit_error("Artifact must be a file, not a directory: {artifact.path}")

  # 2. Validate required context
  for slot_name, slot in self.context_schema:
    if slot.required and slot_name not in context.items:
      exit_error("Missing required context '{slot_name}' for gate '{self.name}'")

  # 3. Warn on unexpected context
  for item_name in context.items:
    if item_name not in self.context_schema:
      warn_stderr("Unknown context item '{item_name}' for gate '{self.name}' — ignored")

  # 4. Validate context paths
  for item_name, item in context.items:
    if item.path is not null and not file_exists(item.path):
      exit_error("Context '{item_name}' path not found: {item.path}")
    if item.path is not null and is_directory(item.path):
      exit_error("Context '{item_name}' must be a file, not a directory: {item.path}")

  # ── Compute hashes ──
  artifact_hash = sha256(artifact.content)
  context_hash = compute_context_hash(context)
  # See Section 10 for context_hash computation details.

  # ── Determine suppressed statuses ──
  suppressed = options.suppressed_statuses   # set of strings, e.g. {"error"}, {"warn"}, or {"error", "warn"}

  # ── Run validators ──
  results = {}
  warnings = []

  for validator in self.validators:
    # Apply filters
    if options.only is not null and validator.name not in options.only:
      results[validator.name] = ValidatorResult(name=validator.name, status="skip", ...)
      continue
    if options.skip is not null and validator.name in options.skip:
      results[validator.name] = ValidatorResult(name=validator.name, status="skip", ...)
      continue
    if options.tags is not null and not any(t in validator.tags for t in options.tags):
      results[validator.name] = ValidatorResult(name=validator.name, status="skip", ...)
      continue

    result = execute_validator(validator, artifact, context, results)
    results[validator.name] = result

    if result.status == "warn":
      warnings.append(validator.name)

    # Determine effective status (apply suppression)
    effective_status = "pass" if result.status in suppressed else result.status

    if effective_status in ["fail", "error"] and validator.blocking and not options.run_all:
      # Pipeline halts
      return Verdict(
        status = effective_status,
        gate = self.name,
        failed_at = validator.name,
        feedback = result.feedback,
        duration_ms = elapsed_ms(run_start),
        timestamp = now_iso8601(),
        artifact_hash = artifact_hash,
        context_hash = context_hash,
        warnings = warnings,
        suppressed = list(suppressed),
        history = list(results.values())
      )

  # ── Compute final status ──
  if options.run_all:
    final_status = compute_final_status(list(results.values()), suppressed)
  else:
    # In normal mode, if we reach here, no blocking validator failed/errored
    # (after suppression). Non-blocking failures do not affect the verdict.
    final_status = "pass"

  failed_at = null
  feedback = null
  if final_status in ["fail", "error"]:
    # Find the first failure/error for reporting
    for r in results.values():
      if r.status == final_status:
        failed_at = r.name
        feedback = r.feedback
        break

  return Verdict(
    status = final_status,
    gate = self.name,
    failed_at = failed_at,
    feedback = feedback,
    duration_ms = elapsed_ms(run_start),
    timestamp = now_iso8601(),
    artifact_hash = artifact_hash,
    context_hash = context_hash,
    warnings = warnings,
    suppressed = list(suppressed),
    history = list(results.values())
  )
```

### 6.7 — Signal Handling and Cleanup

Baton handles process signals to ensure clean shutdown and resource cleanup.

**SIGINT (Ctrl+C) — Graceful shutdown:**

1. Set a `shutting_down` flag.
2. If a validator is currently executing:
   - For `script`: send SIGTERM to the child process group.
   - For `llm` (completion): cancel the in-flight HTTP request.
   - For `llm` (session): call the runtime's `cancel` hook.
3. Wait up to 10 seconds for the current validator to finish or terminate.
4. Record the current validator as `error` with feedback `"[baton] Interrupted by user (SIGINT)"`.
5. Write a partial verdict to the history database with status `"error"`.
6. Clean up temporary files in `tmp_dir`.
7. Exit with code 2.

**Second SIGINT (Ctrl+C again) — Immediate shutdown:**

1. Skip waiting for the current validator.
2. Write a minimal partial verdict if possible.
3. Exit immediately with code 2. Temporary files may be left behind.

**SIGTERM — Same as first SIGINT.** Treat as a graceful shutdown request.

**Stale temporary files:**

The `baton clean` command (see [Section 9](#9-cli-interface)) removes any leftover files in `tmp_dir` and cleans up orphaned agent sessions if possible. This handles cases where baton was killed without graceful shutdown.

```
function cleanup_temp_files(tmp_dir: string):
  if not directory_exists(tmp_dir):
    return

  for file in list_files(tmp_dir):
    # Only remove files older than 1 hour (avoid racing with concurrent runs)
    if file.modified_time < now() - 1 hour:
      delete(file)
      log_debug("Cleaned up stale temp file: {file.path}")
```

---

## 7. Runtime Hooks

### 7.1 — Why Agent Sessions

LLM validators in `completion` mode send a single prompt and receive a single response. This works for structured, bounded review tasks. But some validation tasks benefit from multi-step reasoning: running the code, exploring edge cases, checking interactions between components, or performing analysis that exceeds a single prompt-response cycle.

LLM validators with `mode = "session"` launch a full agent session — with tool use, file access, and iterative reasoning — dedicated to the validation task. The agent can read files, execute code, run tests with novel inputs, and produce a verdict grounded in observation rather than static analysis.

### 7.2 — Runtime Hook Interface

Baton defines a set of lifecycle hooks that runtime adapters implement. The interface is designed to be thin and translatable across different agent frameworks (OpenHands, SWE-agent, Aider, custom agents, or any REST/subprocess-based runtime).

```
Interface RuntimeAdapter:
  # Verify the runtime is reachable and healthy.
  # Called by `baton check-runtime`.
  method health_check() → HealthResult

  # Create a new isolated session and submit the validation task.
  # Returns a handle that can be used to poll or cancel.
  method create_session(config: SessionConfig) → SessionHandle

  # Poll the session for completion. Returns the current state.
  # Implementations may also support streaming/callback patterns,
  # but polling is the minimum required interface.
  method poll_status(handle: SessionHandle) → SessionStatus

  # Collect the final result after the session completes.
  # Must only be called when poll_status returns a terminal state.
  method collect_result(handle: SessionHandle) → SessionResult

  # Cancel a running session. Used on timeout or SIGINT.
  # Must be idempotent (safe to call multiple times or on an already-finished session).
  method cancel(handle: SessionHandle) → void

  # Clean up session resources (temp files, containers, etc.).
  # Called after collect_result or cancel.
  # Must be idempotent.
  method teardown(handle: SessionHandle) → void


Record SessionConfig:
  task:            string               # the rendered prompt for the agent
  files:           map[string → path]   # artifact + context files to mount
  model:           string               # model the agent should use
  sandbox:         boolean              # whether to sandbox the environment
  max_iterations:  integer              # cap on tool-use loops
  timeout_seconds: integer              # session wall-clock timeout
  env:             map[string → string] # additional environment variables


Record HealthResult:
  reachable:  boolean
  version:    string | null     # runtime version, if available
  models:     list[string] | null  # available models, if queryable
  message:    string | null     # human-readable status


Enum SessionStatus:
  RUNNING       # session is still executing
  COMPLETED     # session finished normally
  FAILED        # session encountered an error
  TIMED_OUT     # session exceeded its timeout
  CANCELLED     # session was cancelled


Record SessionResult:
  status:    "completed" | "failed" | "timed_out" | "cancelled"
  output:    string          # the agent's final textual output
  raw_log:   string          # full session log (for debugging)
  cost:      Cost | null     # token/dollar cost, if available
```

### 7.3 — Session Lifecycle

When baton runs an `llm` validator with `mode = "session"`:

```
function execute_llm_session_validator(
  validator: ValidatorConfig,
  artifact: Artifact,
  context: Context,
  prior_results: map[string → ValidatorResult]
) → ValidatorResult:

  runtime = get_runtime_adapter(validator.runtime)
  prompt = resolve_prompt(validator.prompt, artifact, context, prior_results)

  # ── Prepare isolated file set ──
  files = {"artifact": artifact.path}
  for ref in validator.context_refs:
    if ref in context.items and context.items[ref].path is not null:
      files[ref] = context.items[ref].path

  # ── Create session ──
  try:
    handle = runtime.create_session(SessionConfig(
      task = prompt,
      files = files,
      model = validator.model or runtime.default_model,
      sandbox = validator.sandbox if validator.sandbox is not null else runtime.sandbox,
      max_iterations = validator.max_iterations or runtime.max_iterations,
      timeout_seconds = validator.timeout_seconds or runtime.timeout_seconds,
      env = {}
    ))
  catch Exception as e:
    return ValidatorResult(
      name = validator.name,
      status = "error",
      feedback = "[baton] Failed to create session on runtime '{validator.runtime}': {e}"
    )

  # ── Poll until terminal ──
  try:
    loop:
      status = runtime.poll_status(handle)
      if status in [COMPLETED, FAILED, TIMED_OUT, CANCELLED]:
        break
      sleep(poll_interval)  # implementation-defined, e.g. 2 seconds
  catch Exception as e:
    runtime.cancel(handle)
    runtime.teardown(handle)
    return ValidatorResult(
      name = validator.name,
      status = "error",
      feedback = "[baton] Error polling session: {e}"
    )

  # ── Collect result ──
  result = runtime.collect_result(handle)

  # Store raw log for debugging (not in verdict)
  store_session_log(validator.name, handle.id, result.raw_log)

  # ── Parse verdict from agent output ──
  runtime.teardown(handle)

  if result.status != "completed":
    return ValidatorResult(
      name = validator.name,
      status = "error",
      feedback = "[baton] Agent session ended with status '{result.status}'",
      cost = result.cost
    )

  parsed = parse_verdict(result.output)
  return ValidatorResult(
    name = validator.name,
    status = parsed.status,
    feedback = parsed.evidence,
    cost = result.cost
  )
```

### 7.4 — Isolation Guarantees

Session-mode validators maintain baton's context isolation principles:

- The agent session has **no access** to the producing agent's conversation, tools, or environment.
- The agent session has **no network access** beyond what the runtime's sandbox policy allows. (Default: no network.)
- Files mounted into the session are **copies**, not references. The agent cannot modify the original artifact or context.
- The session is **ephemeral**. No state persists between validator invocations.

### 7.5 — Writing a Custom Adapter

To support a new agent runtime, implement the `RuntimeAdapter` interface. The adapter must:

1. Map `SessionConfig` fields to the runtime's native configuration.
2. Mount the `files` map into the session's workspace. The runtime must not expose any other files.
3. Return the agent's final textual output via `collect_result`. Baton handles verdict parsing.
4. Implement `cancel` and `teardown` as idempotent operations.

Register the adapter by adding a `[runtimes.<n>]` section to `baton.toml` with `type = "<your_adapter_type>"`. Baton's adapter registry maps the `type` string to the adapter implementation.

**Testing adapters:** See [Section 13](#13-testing-strategy) for guidance on testing runtime adapters with mock sessions.

---

## 8. Reference Adapter: OpenHands

> **Note:** This section is implementation detail for the OpenHands runtime adapter. It can be replaced wholesale with a section for a different runtime without affecting the rest of the spec. The stable contract is the `RuntimeAdapter` interface in [Section 7](#7-runtime-hooks).

### 8.1 — OpenHands Adapter Implementation

The OpenHands adapter implements `RuntimeAdapter` by calling the OpenHands API:

```
OpenHandsAdapter implements RuntimeAdapter:

  constructor(config: RuntimeConfig):
    self.base_url = config.base_url
    self.api_key = resolve_env(config.api_key_env)  # null if no auth
    self.default_model = config.default_model
    self.sandbox = config.sandbox
    self.timeout_seconds = config.timeout_seconds
    self.max_iterations = config.max_iterations

  method health_check() → HealthResult:
    try:
      response = http_get("{self.base_url}/api/health",
        headers = self.auth_headers())
      if response.status == 200:
        return HealthResult(reachable=true, version=response.body.version, ...)
      else:
        return HealthResult(reachable=false, message="HTTP {response.status}")
    catch Exception as e:
      return HealthResult(reachable=false, message=str(e))

  method create_session(config: SessionConfig) → SessionHandle:
    # Upload files to workspace
    workspace_id = generate_uuid()
    for name, path in config.files:
      upload_file("{self.base_url}/api/workspaces/{workspace_id}/files",
        file_path = path,
        target_name = name,
        headers = self.auth_headers())

    # Create session
    response = http_post("{self.base_url}/api/sessions",
      headers = self.auth_headers(),
      body = {
        workspace_id: workspace_id,
        task: config.task,
        model: config.model,
        sandbox: config.sandbox,
        max_iterations: config.max_iterations,
        timeout: config.timeout_seconds
      })

    return SessionHandle(
      id = response.body.session_id,
      workspace_id = workspace_id
    )

  method poll_status(handle: SessionHandle) → SessionStatus:
    response = http_get(
      "{self.base_url}/api/sessions/{handle.id}/status",
      headers = self.auth_headers())
    # Map OpenHands-specific statuses to SessionStatus enum
    return map_openhands_status(response.body.status)

  method collect_result(handle: SessionHandle) → SessionResult:
    response = http_get(
      "{self.base_url}/api/sessions/{handle.id}/result",
      headers = self.auth_headers())
    return SessionResult(
      status = map_openhands_status(response.body.status),
      output = response.body.final_message,
      raw_log = response.body.full_log,
      cost = extract_cost_from_openhands(response.body.metrics)
    )

  method cancel(handle: SessionHandle) → void:
    http_delete(
      "{self.base_url}/api/sessions/{handle.id}",
      headers = self.auth_headers())

  method teardown(handle: SessionHandle) → void:
    # Clean up workspace
    http_delete(
      "{self.base_url}/api/workspaces/{handle.workspace_id}",
      headers = self.auth_headers())

  # ── Private helpers ──

  method auth_headers() → map:
    if self.api_key is not null:
      return {"Authorization": "Bearer {self.api_key}"}
    return {}
```

### 8.2 — OpenHands-Specific Considerations

- **Model availability:** OpenHands sessions use their own model configuration. The `model` field in the session config is passed to OpenHands, which must have access to that model's API. Verify with `baton check-runtime openhands`.
- **Sandbox:** When `sandbox = true`, OpenHands runs the agent in a Docker container. This requires Docker to be available on the host. If Docker is not available and `sandbox = true`, session creation fails with an error.
- **Iteration limits:** The `max_iterations` cap prevents runaway agent loops. If the agent hits this limit without producing a verdict, baton records an `error`.
- **Network isolation:** Sandboxed sessions have no network access by default. If a validator needs network access (e.g., to fetch dependencies for testing), the runtime's sandbox policy must be configured to allow it. This is an OpenHands configuration concern, not a baton concern.

---

## 9. CLI Interface

### 9.1 — Commands

**`baton init`**

Scaffolds a new baton project in the current directory.

```bash
baton init
```

Creates:
```
.baton/
├── history.db          # empty SQLite database
├── logs/               # directory for verdict logs
├── tmp/                # directory for temporary files
baton.toml              # starter config with one example gate
prompts/
├── spec-compliance.md  # starter prompt template
├── adversarial-review.md
├── doc-completeness.md
```

The generated `baton.toml` includes a commented-out example gate and provider configuration. The prompt templates are the starters from [Appendix A](#appendix-a-prompt-template-library-starters).

Flags:
- `--minimal` — Only creates `baton.toml` and `.baton/` directory, no prompt templates.
- `--prompts-only` — Only creates the `prompts/` directory with starter templates.

Edge cases:
- If `baton.toml` already exists, print an error and exit. Do not overwrite.
- If `.baton/` already exists, print a warning and continue (only create missing subdirectories).

**`baton check`**

Runs a gate against an artifact. This is the primary command.

```bash
baton check \
  --gate code-review \
  --artifact ./output/patch.diff \
  --context spec=./tasks/issue-42.md \
  --context prior_output=./output/plan.md
```

Flags:
- `--config <path>` — Path to `baton.toml`. Default: searches upward from cwd.
- `--gate <n>` — Gate to run. Required.
- `--artifact <path>` — Path to the artifact. Required. Use `-` for stdin.
- `--context <n>=<path>` — Context items. Repeatable. `<path>` can be a file path or `-` for stdin (only one context item may use stdin).
- `--all` — Run all validators even if a blocking one fails. Final verdict reflects the worst outcome across all validators, including non-blocking ones (see [Section 6.1](#61--pipeline-execution)).
- `--only <n>[,<n>...]` — Run only the named validators (comma-separated or repeated flag). Skips others.
- `--tags <tag>[,<tag>...]` — Run only validators matching the given tags. Validators with no `tags` field (or an empty `tags` array) are skipped when this flag is used.
- `--skip <n>[,<n>...]` — Skip the named validators.
- `--timeout <seconds>` — Override default timeout for all validators.
- `--format <json|human|summary>` — Output format. Default: `json` if stdout is not a TTY, `human` otherwise.
- `--dry-run` — Print the validators that would run (after applying `--only`, `--skip`, `--tags`, and `run_if` evaluation) and exit. When combined with `--all`, all validators are shown (since `--all` disables blocking-based halting), but `run_if` conditions that depend on runtime results are shown as `"(depends on runtime)"` rather than evaluated.
- `--no-log` — Don't write to the history database or log files.
- `--verbose` / `-v` — Print each validator's result as it completes (to stderr).
- `--suppress-warnings` — Treat `warn` statuses as `pass` for verdict computation. Warnings are still recorded with their true status in the verdict `history` array. The verdict's `suppressed` field will include `"warn"`.
- `--suppress-errors` — Treat `error` statuses as `pass` for verdict computation. Errors are still recorded with their true status in the verdict `history` array. The verdict's `suppressed` field will include `"error"`. When a blocking validator errors with this flag active, the pipeline does **not** halt (the error is treated as a pass for flow control purposes).
- `--suppress-all` — Equivalent to `--suppress-warnings --suppress-errors`.

**Suppression semantics:**

Suppression affects how individual validator statuses contribute to the gate verdict, but does not alter the recorded status of individual validators. The `history` array always contains the true status. Suppression interacts with `--all` mode and the normal `error > fail` precedence rule as follows:

| Scenario (`--all` mode) | No suppression | `--suppress-errors` | `--suppress-warnings` | `--suppress-all` |
|---|---|---|---|---|
| error + fail | error | fail | error | pass |
| error + pass only | error | pass | error | pass |
| fail + warn only | fail | fail | fail | pass |
| warn + pass only | pass | pass | pass | pass |
| error only | error | pass | error | pass |
| fail only | fail | fail | fail | pass |

This is useful when a validator is known to be broken (e.g., a missing API key) and the user wants to skip it without removing it from the config file. The `--suppress-errors` flag lets the gate proceed as if that validator passed, while still recording the error for observability.

Edge cases:
- If both `--artifact -` and `--context foo=-` are used, print an error. Only one input can be stdin.
- If `--context foo=-` and `--context bar=-` are both used, print an error. Only one context item may read from stdin.
- If `--only` and `--skip` both name the same validator, `--skip` wins (the validator is skipped).
- If `--only` names a validator that does not exist in the gate, print an error and exit.
- If `--skip` names a validator that does not exist in the gate, print a warning to stderr and continue. This is more lenient than `--only` because skipping a nonexistent validator is a no-op, while running a nonexistent validator is an error.
- If `--gate` names a gate that does not exist, print an error and list available gates.

**`baton list`**

Lists available gates and their validators.

```bash
baton list                      # list all gates
baton list --gate code-review   # show validators in a gate
```

**`baton history`**

Queries the verdict history database.

```bash
baton history                           # recent verdicts
baton history --gate code-review        # filter by gate
baton history --status fail             # filter by status
baton history --since 2026-03-01        # filter by date
baton history --artifact-hash abc123    # find verdicts for a specific artifact
baton history --export csv              # export as CSV
```

**`baton validate-config`**

Parses `baton.toml` and checks for errors without running anything.

```bash
baton validate-config
baton validate-config --config path/to/baton.toml
```

Checks (exhaustive list):

| Check | Severity | Message |
|---|---|---|
| TOML syntax error | error | `"TOML parse error at line {n}: {detail}"` |
| Missing `version` field | error | `"Missing required field: version"` |
| Unknown version | error | `"Unsupported version '{v}'. Expected '0.4'."` |
| No gates defined | error | `"No gates defined. At least one gate is required."` |
| Gate with no validators | error | `"Gate '{name}' has no validators."` |
| Duplicate validator name in gate | error | `"Gate '{gate}': duplicate validator name '{name}'."` |
| Validator name invalid chars | error | `"Validator name '{name}' contains invalid characters. Must match [A-Za-z0-9_-]+."` |
| Unknown validator type | error | `"Validator '{name}': unknown type '{type}'. Expected 'script', 'llm', or 'human'."` |
| Missing required validator field | error | `"Validator '{name}': missing required field '{field}'."` |
| `run_if` references nonexistent validator | error | `"Validator '{name}': run_if references unknown validator '{ref}'."` |
| `run_if` forward reference | error | `"Validator '{name}': run_if references '{ref}' which appears later in the pipeline."` |
| `run_if` syntax error | error | `"Validator '{name}': invalid run_if expression: '{expr}'."` |
| `context_refs` references undefined context | error | `"Validator '{name}': context_refs includes '{ref}' which is not defined on gate '{gate}'."` |
| Prompt file not found | error | `"Validator '{name}': prompt file not found: '{path}'."` |
| Prompt file empty | error | `"Validator '{name}': prompt file is empty: '{path}'."` |
| Provider key not defined | error | `"Validator '{name}': provider '{key}' is not defined in [providers]."` |
| Runtime key not defined | error | `"Validator '{name}': runtime '{key}' is not defined in [runtimes]."` |
| `mode = "session"` without `runtime` | error | `"Validator '{name}': mode 'session' requires a 'runtime' field."` |
| `mode = "completion"` with `runtime` | warning | `"Validator '{name}': runtime field ignored in completion mode."` |
| `api_key_env` references unset variable | error | `"Provider '{name}': env var '{var}' is not set."` |
| Unresolved `${VAR}` in config | error | `"Unresolved variable '${VAR}' in {location}."` |
| Prompt filename has unusual chars | warning | `"Prompt file '{name}' contains non-portable characters."` |
| Template `expects` mismatches validator `response_format` | error | `"Validator '{name}': response_format is '{rf}' but template expects '{te}'."` |
| `blocking = true` on freeform validator | warning | `"Validator '{name}': blocking has no effect with response_format 'freeform' (freeform always returns warn)."` |
| `warn_exit_codes` contains 0 | error | `"Validator '{name}': warn_exit_codes must not contain 0 (exit code 0 is always pass)."` |

**`baton check-provider`**

Verifies that a provider is reachable and the configured model exists.

```bash
baton check-provider              # check the default provider
baton check-provider openai       # check a named provider
baton check-provider --all        # check all configured providers
```

Behavior:
1. Resolve the provider's `api_base` and `api_key_env`.
2. Send a minimal request (e.g., list models or a trivial completion) to verify connectivity.
3. If a `default_model` is configured, verify that the model is available.
4. Print results to stdout.

```
function check_provider(provider_name: string, config: ProviderConfig):
  # 1. Check API key
  api_key = resolve_env(config.api_key_env)
  if config.api_key_env != "" and api_key is null:
    print_error("API key env var '{config.api_key_env}' is not set")
    return

  # 2. Check connectivity
  try:
    response = http_get("{config.api_base}/v1/models",
      headers = auth_headers(api_key),
      timeout = 10)
  catch Exception as e:
    print_error("Cannot reach {config.api_base}: {e}")
    return

  if response.status == 401 or response.status == 403:
    print_error("Authentication failed. Check {config.api_key_env}.")
    return

  # 3. Check model availability
  if response.status == 200:
    models = parse_model_list(response.body)
    if config.default_model in models:
      print_ok("Provider '{provider_name}': reachable, model '{config.default_model}' available")
    else:
      print_warn("Provider '{provider_name}': reachable, but model '{config.default_model}' not found")
      print_warn("Available models: {models[:10]}")
  else:
    # Some providers don't support /v1/models — try a minimal completion
    print_warn("Model list not available. Attempting test completion...")
    try:
      test_response = http_post("{config.api_base}/v1/chat/completions",
        headers = auth_headers(api_key),
        body = {model: config.default_model, messages: [{role: "user", content: "ping"}], max_tokens: 1},
        timeout = 15)
      if test_response.status == 200:
        print_ok("Provider '{provider_name}': reachable, model '{config.default_model}' responds")
      else:
        print_error("Provider '{provider_name}': HTTP {test_response.status}")
    catch Exception as e:
      print_error("Provider '{provider_name}': test completion failed: {e}")
```

**`baton check-runtime`**

Verifies that an agent runtime is reachable and healthy.

```bash
baton check-runtime openhands     # check a named runtime
baton check-runtime --all         # check all configured runtimes
```

Calls the runtime adapter's `health_check()` method and prints the result.

**`baton clean`**

Removes stale temporary files and reports any orphaned resources.

```bash
baton clean               # clean up tmp_dir
baton clean --dry-run     # show what would be cleaned without deleting
```

Behavior:
1. Scan `tmp_dir` for files older than 1 hour.
2. Remove them (or list them if `--dry-run`).
3. If any configured runtimes support session listing, check for orphaned sessions and report them.

**`baton version`**

Prints baton version, spec version, and config file location.

```bash
baton version
```

Output:
```
baton 0.4.0
spec version: 0.4
config: /home/user/project/baton.toml (found)
```

The tool version (e.g., `0.4.0`) and spec version (e.g., `0.4`) may diverge. The tool version tracks implementation releases (including patch fixes that don't change the spec). The spec version is what appears in `baton.toml`'s `version` field and determines which features and behaviors are available. A tool version of `0.4.3` still implements spec `0.4`.

### 9.2 — Config File Discovery

When `--config` is not specified, baton searches for `baton.toml` in the current directory, then each parent directory, stopping at the filesystem root or a `.git` directory (whichever comes first). This mirrors the behavior of tools like `pyproject.toml` and `Cargo.toml`.

**Edge cases:**
- If no `baton.toml` is found anywhere in the search path, baton prints an error and exits. The error message includes the search path for debugging.
- If `--config` points to a file that does not exist, baton prints an error and exits.
- Symlinks are followed during the search.

### 9.3 — Standard Input

When `--artifact -` is passed, baton reads the artifact from stdin, writes it to a temporary file (in `tmp_dir`), and passes that file to validators. This enables piping:

```bash
git diff --cached | baton check --gate code-review --artifact - --context spec=./SPEC.md
```

The temporary file is cleaned up after the gate completes. If baton is interrupted, `baton clean` handles leftover files.

### 9.4 — Examples

```bash
# Basic: validate a file
baton check --gate code-review --artifact ./patch.diff --context spec=./spec.md

# Run only the LLM validators (by tag)
baton check --gate code-review --artifact ./patch.diff --context spec=./spec.md \
  --tags deep

# Run all validators, don't stop on failure
baton check --gate code-review --artifact ./patch.diff --context spec=./spec.md --all

# Run a single validator by name
baton check --gate code-review --artifact ./patch.diff --context spec=./spec.md \
  --only adversarial-review

# Dry run: see what would execute
baton check --gate code-review --artifact ./patch.diff --context spec=./spec.md --dry-run

# Pipe from git
git diff HEAD~1 | baton check --gate code-review --artifact - --context spec=./spec.md

# In CI
baton check --gate code-review --artifact $PATCH_PATH --context spec=./spec.md --format json

# Skip a broken validator (missing API key) without removing it from config
baton check --gate code-review --artifact ./patch.diff --context spec=./spec.md \
  --all --suppress-errors

# Verify your setup
baton validate-config
baton check-provider --all
baton check-runtime --all
```

---

## 10. Library Interface (Pseudocode)

This section defines the public API that any implementation must expose. The pseudocode is language-agnostic; implementations should map these to idiomatic constructs in their target language.

### 10.1 — Core Types

```
Record Artifact:
  path:     string | null      # file path, if file-backed
  content:  bytes | null       # raw content, if in-memory
  hash:     string             # SHA-256, computed lazily

  static method from_file(path: string) → Artifact:
    if not file_exists(path):
      error("Artifact file not found: {path}")
    if is_directory(path):
      error("Artifact must be a file, not a directory: {path}")
    return Artifact(path=path, content=null, hash=null)

  static method from_string(content: string) → Artifact:
    return Artifact(path=null, content=encode_utf8(content), hash=null)

  static method from_bytes(content: bytes) → Artifact:
    return Artifact(path=null, content=content, hash=null)

  method get_content() → bytes:
    if self.content is null:
      self.content = read_file_bytes(self.path)
    return self.content

  method get_hash() → string:
    if self.hash is null:
      self.hash = sha256_hex(self.get_content())
    return self.hash


Record Context:
  items: map[string → ContextItem]

  static method from_map(m: map[string → string | path]) → Context:
    items = {}
    for name, value in m:
      if file_exists(value):
        if is_directory(value):
          error("Context '{name}' must be a file, not a directory: {value}")
        items[name] = ContextItem(name=name, path=value, content=null)
      else:
        # Treat as inline string
        items[name] = ContextItem(name=name, path=null, content=value)
    return Context(items=items)

  method get_hash() → string:
    # Deterministic: sort by name, hash each item, join with ":" separator, hash again.
    # If no context items are provided (empty map), the hash is sha256_hex("")
    # (the SHA-256 of the empty string). This is a stable, well-defined value.
    item_hashes = []
    for name in sorted(self.items.keys()):
      item = self.items[name]
      item_hashes.append(sha256_hex(item.get_content()))
    return sha256_hex(join(item_hashes, separator=":"))


Record ContextItem:
  name:    string
  path:    string | null
  content: string | null       # string (text). Context items are always text, unlike
                               # artifacts which are bytes. Binary context is not supported;
                               # callers should base64-encode or reference by path if needed.

  method get_content() → string:
    if self.content is null:
      self.content = read_file_text(self.path)
    return self.content

  method get_hash() → string:
    return sha256_hex(encode_utf8(self.get_content()))


Record Verdict:
  status:        "pass" | "fail" | "error"
  gate:          string
  failed_at:     string | null
  feedback:      string | null
  duration_ms:   integer
  timestamp:     datetime
  artifact_hash: string
  context_hash:  string
  warnings:      list[string]
  suppressed:    list[string]
  history:       list[ValidatorResult]

  method to_json() → string:
    # Serialize to JSON. All fields included.
    # history entries include their full ValidatorResult.

  method to_human() → string:
    # Human-readable summary for stderr.
    # Example:
    #   ✓ lint (12ms)
    #   ✓ typecheck (340ms)
    #   ✗ spec-compliance (1204ms)
    #     FAIL: Section 3.2 requires pagination support, not implemented.
    #   — adversarial-review (skipped)
    #   VERDICT: fail (failed at: spec-compliance)

  method to_summary() → string:
    # One-line summary.
    # Example: "FAIL at spec-compliance: Section 3.2 requires pagination support"


Record ValidatorResult:
  name:         string
  status:       "pass" | "fail" | "warn" | "skip" | "error"
  feedback:     string | null
  duration_ms:  integer
  cost:         Cost | null


Record Cost:
  input_tokens:  integer | null
  output_tokens: integer | null
  model:         string | null
  estimated_usd: float | null
```

### 10.2 — Gate

```
Class Gate:
  properties:
    name:           string
    description:    string
    validators:     list[ValidatorConfig]
    context_schema: map[string → ContextSlot]

  static method from_config(path: string, gate_name: string) → Gate:
    # 1. Find and parse baton.toml (using config discovery if path is null)
    # 2. Validate the full config (same checks as baton validate-config)
    # 3. Resolve the named gate
    # 4. Instantiate validator configs (resolve prompt files, verify providers)
    # If gate_name not found: error with list of available gates.

  method run(artifact: Artifact, context: Context, options: RunOptions) → Verdict:
    # Full implementation in Section 6.6.
    # Returns a Verdict. Logs to history_db unless options.log is false.


Record RunOptions:
  run_all:              boolean            # ignore blocking, run everything
  only:                 set[string] | null # only run these validators
  skip:                 set[string] | null # skip these validators
  tags:                 set[string] | null # only run validators with these tags
  timeout:              integer | null     # override default timeout
  log:                  boolean            # whether to persist verdict. Default: true
  suppressed_statuses:  set[string]        # statuses to suppress (e.g., {"error"})
```

### 10.3 — Validator Dispatch

Each validator type implements a common interface:

```
Interface Validator:
  property name: string
  property blocking: boolean
  property run_if: string | null
  property tags: list[string]

  method execute(artifact: Artifact, context: Context, prior_results: map[string → ValidatorResult]) → ValidatorResult
```

**ScriptValidator:**

```
method execute(artifact, context, prior_results):
  command = self.command
  command = replace_placeholders(command, artifact, context, prior_results)

  # Validate command is not empty after placeholder resolution
  if command.strip() is empty:
    return ValidatorResult(name=self.name, status="error",
      feedback="[baton] Command is empty after placeholder resolution")

  try:
    process = spawn_shell(command,
      working_dir = self.working_dir or dirname(artifact.path) or cwd(),
      env = merge(os.environ, self.env),
      timeout = self.timeout_seconds)

    exit_code, stdout, stderr = process.wait()
  catch CommandNotFound:
    return ValidatorResult(name=self.name, status="error",
      feedback="[baton] Command not found: {command.split()[0]}")
  catch PermissionDenied:
    return ValidatorResult(name=self.name, status="error",
      feedback="[baton] Permission denied: {command}")
  catch TimeoutError:
    # SIGTERM the process group, wait 5s, then SIGKILL
    process.terminate()
    sleep(5)
    if process.is_alive():
      process.kill()
    return ValidatorResult(name=self.name, status="error",
      feedback="[baton] Validator timed out after {self.timeout_seconds} seconds")

  if exit_code == 0:
    return ValidatorResult(name=self.name, status="pass", feedback=null)
  else if exit_code in self.warn_exit_codes:
    feedback = (stdout + "\n" + stderr).strip()
    if len(feedback) == 0:
      feedback = "[baton] Script exited with code {exit_code} (warn, no output)"
    return ValidatorResult(name=self.name, status="warn", feedback=feedback)
  else:
    feedback = (stdout + "\n" + stderr).strip()
    if len(feedback) == 0:
      feedback = "[baton] Script exited with code {exit_code} (no output)"
    return ValidatorResult(name=self.name, status="fail", feedback=feedback)
```

**LLMCompletionValidator:**

```
method execute(artifact, context, prior_results):
  prompt = resolve_prompt(self.prompt, artifact, context, prior_results)
  provider = get_provider(self.provider)

  try:
    response = provider.complete(
      model = self.model or provider.default_model,
      messages = [
        {role: "system", content: self.system_prompt or DEFAULT_SYSTEM_PROMPT},
        {role: "user", content: prompt}
      ],
      temperature = self.temperature,
      max_tokens = self.max_tokens
    )
  catch AuthenticationError:
    return ValidatorResult(name=self.name, status="error",
      feedback="[baton] Authentication failed for provider '{self.provider}'. Check {provider.api_key_env}.")
  catch ModelNotFoundError:
    return ValidatorResult(name=self.name, status="error",
      feedback="[baton] Model '{self.model}' not found on provider '{self.provider}'.")
  catch RateLimitError as e:
    return ValidatorResult(name=self.name, status="error",
      feedback="[baton] Rate limited by provider '{self.provider}'. Retry-After: {e.retry_after}s")
  catch ContextLengthError as e:
    return ValidatorResult(name=self.name, status="error",
      feedback="[baton] Input exceeds context window for model '{self.model}' ({e.token_count} tokens).")
  catch ConnectionError as e:
    return ValidatorResult(name=self.name, status="error",
      feedback="[baton] Cannot reach provider '{self.provider}' at {provider.api_base}: {e}")

  text = response.content
  cost = Cost(
    input_tokens = response.usage.input_tokens,
    output_tokens = response.usage.output_tokens,
    model = self.model or provider.default_model,
    estimated_usd = null  # optional: compute from known pricing
  )

  if text is null or text.strip() is empty:
    return ValidatorResult(name=self.name, status="error",
      feedback="[baton] Provider returned empty or malformed response.",
      cost=cost)

  if self.response_format == "verdict":
    parsed = parse_verdict(text)
    return ValidatorResult(
      name = self.name,
      status = parsed.status,
      feedback = parsed.evidence,
      cost = cost
    )
  else:  # freeform
    # Freeform responses are informational. Status is always "warn" —
    # the output is captured as feedback for the caller to interpret.
    # This means blocking = true has no effect on freeform validators,
    # since warn never halts the pipeline.
    return ValidatorResult(
      name = self.name,
      status = "warn",
      feedback = text,
      cost = cost
    )
```

**HumanValidator:**

```
method execute(artifact, context, prior_results):
  prompt = replace_placeholders(self.prompt, artifact, context, prior_results)
  # Human validators immediately fail with the prompt as feedback.
  # The calling system presents this to a human and decides next steps.
  return ValidatorResult(
    name = self.name,
    status = "fail",
    feedback = "[human-review-requested] " + prompt
  )
```

### 10.4 — Verdict Parsing

The `parse_verdict` function extracts structured results from LLM/agent text output:

```
function parse_verdict(text: string) → ParsedVerdict:
  # Strategy: look for verdict keywords. Prefer the first line, fall back
  # to scanning the full text (agents may produce reasoning before verdicts).

  lines = text.strip().split("\n")

  # ── Pass 1: Check first non-empty line ──
  first_line = null
  for line in lines:
    if line.strip() is not empty:
      first_line = line.strip()
      break

  if first_line is null:
    return ParsedVerdict(status="error",
      evidence="[baton] Validator produced empty output")

  first_upper = first_line.upper()

  # Word-boundary check: the keyword must be followed by end-of-string,
  # whitespace, or punctuation. This prevents "PASSWORD" from matching "PASS".
  # The helper `starts_with_keyword(text, keyword)` returns true if `text`
  # starts with `keyword` and the next character (if any) is not alphanumeric.

  if starts_with_keyword(first_upper, "PASS"):
    return ParsedVerdict(status="pass", evidence=null)

  if starts_with_keyword(first_upper, "WARN"):
    evidence = first_line[4:].strip()  # skip "WARN"
    if evidence is empty:
      evidence = text.strip()[len(first_line):].strip()
    return ParsedVerdict(status="warn", evidence=evidence or null)

  if starts_with_keyword(first_upper, "FAIL"):
    evidence = first_line[4:].strip()  # skip "FAIL"
    if evidence is empty:
      evidence = text.strip()[len(first_line):].strip()
    return ParsedVerdict(status="fail", evidence=evidence or null)

  # ── Pass 2: Scan full text for last verdict keyword ──
  # This handles cases where the LLM produces reasoning before the verdict.
  # Uses word-boundary matching: the keyword must be preceded by
  # start-of-string or non-alphanumeric, and followed by end-of-string
  # or non-alphanumeric. This prevents matching keywords embedded in
  # other words (e.g., "PASSWORD", "FAILING", "WARNING").

  last_pass = rfind_keyword(text.upper(), "PASS")
  last_fail = rfind_keyword(text.upper(), "FAIL")
  last_warn = rfind_keyword(text.upper(), "WARN")

  # Find the rightmost verdict keyword
  candidates = []
  if last_pass >= 0: candidates.append(("pass", last_pass))
  if last_fail >= 0: candidates.append(("fail", last_fail))
  if last_warn >= 0: candidates.append(("warn", last_warn))

  if len(candidates) == 0:
    return ParsedVerdict(status="error",
      evidence="[baton] Could not parse verdict from validator output:\n" + text[:500])

  # Sort by position descending — take the last one
  candidates.sort(key=lambda c: c[1], reverse=true)
  status, pos = candidates[0]

  keyword_len = 4  # "PASS", "FAIL", "WARN" are all 4 chars
  evidence = text[pos + keyword_len:].strip()

  if status == "pass":
    return ParsedVerdict(status="pass", evidence=null)
  else:
    return ParsedVerdict(status=status, evidence=evidence or text[:500])


# ── Helper functions for word-boundary matching ──

function starts_with_keyword(text: string, keyword: string) → boolean:
  # Returns true if text starts with keyword and the next character
  # (if any) is not alphanumeric.
  if not text.starts_with(keyword):
    return false
  if len(text) == len(keyword):
    return true  # keyword is the entire string
  next_char = text[len(keyword)]
  return not next_char.is_alphanumeric()


function rfind_keyword(text: string, keyword: string) → integer:
  # Returns the position of the last occurrence of keyword in text
  # where it appears as a standalone word (not embedded in another word).
  # Returns -1 if not found.
  pos = len(text)
  while true:
    pos = text.rfind(keyword, end=pos)
    if pos < 0:
      return -1
    # Check preceding character
    if pos > 0 and text[pos - 1].is_alphanumeric():
      continue    # skip this occurrence, search earlier
    # Check following character
    end_pos = pos + len(keyword)
    if end_pos < len(text) and text[end_pos].is_alphanumeric():
      continue
    return pos
  return -1
```

**Edge cases for verdict parsing:**

- If the text contains "PASS" as part of another word (e.g., "PASSWORD", "PASSING"), the word-boundary checks in both `starts_with_keyword` and `rfind_keyword` prevent false matches.
- If both PASS and FAIL appear as standalone keywords in the text (e.g., "This would FAIL if not for X, but it does PASS"), the last keyword wins. This heuristic favors agents that put their verdict at the end of their reasoning.
- Very long validator outputs are truncated to the first 500 characters in error messages to keep verdicts readable.

---

## 11. Verdict History and Logging

### 11.1 — History Database

Baton maintains a local SQLite database (default: `.baton/history.db`) that stores every verdict. 

**Concurrency note:** Multiple concurrent `baton check` invocations (e.g., in parallel CI jobs sharing a workspace) can race on the database. Implementations must:
1. Open the database in WAL (Write-Ahead Logging) mode.
2. Wrap all writes in explicit transactions.
3. Use a short retry with backoff on `SQLITE_BUSY` (e.g., 3 retries, 50ms/100ms/200ms delays).

If this proves insufficient for high-concurrency environments, callers should use `--no-log` and aggregate verdicts externally.

**Schema:**

```sql
-- Enable WAL mode (run once on database creation)
PRAGMA journal_mode=WAL;

CREATE TABLE verdicts (
  id             TEXT PRIMARY KEY,    -- UUID
  timestamp      TEXT NOT NULL,       -- ISO 8601
  gate           TEXT NOT NULL,
  status         TEXT NOT NULL,       -- pass, fail, error
  failed_at      TEXT,
  feedback       TEXT,
  duration_ms    INTEGER NOT NULL,
  artifact_hash  TEXT NOT NULL,
  context_hash   TEXT NOT NULL,
  warnings_json  TEXT,                -- JSON array of validator names
  suppressed_json TEXT,               -- JSON array of suppressed statuses
  verdict_json   TEXT NOT NULL        -- full verdict as JSON
);

CREATE TABLE validator_results (
  id             TEXT PRIMARY KEY,    -- UUID
  verdict_id     TEXT NOT NULL REFERENCES verdicts(id),
  name           TEXT NOT NULL,
  status         TEXT NOT NULL,       -- pass, fail, warn, skip, error
  feedback       TEXT,
  duration_ms    INTEGER NOT NULL,
  input_tokens   INTEGER,
  output_tokens  INTEGER,
  model          TEXT,
  estimated_usd  REAL
);

CREATE INDEX idx_verdicts_gate ON verdicts(gate);
CREATE INDEX idx_verdicts_status ON verdicts(status);
CREATE INDEX idx_verdicts_artifact ON verdicts(artifact_hash);
CREATE INDEX idx_verdicts_context ON verdicts(context_hash);
CREATE INDEX idx_verdicts_timestamp ON verdicts(timestamp);
CREATE INDEX idx_vresults_verdict ON validator_results(verdict_id);
```

### 11.2 — Structured Logs

Each gate invocation writes a JSON log entry to `<log_dir>/<date>/<gate_name>-<timestamp>.json`. The log entry contains:

- The full verdict (same JSON as stdout output)
- The artifact hash and path (not content)
- Context item paths and hashes (not content)
- For session-mode validators: the session ID (raw session logs are stored separately at `<log_dir>/sessions/<session_id>.log`)
- Wall-clock timings for each validator
- Token/cost data for LLM validators

Logs are append-only and never modified. They are intended for debugging and auditing, not queried by baton itself (that's what the SQLite database is for).

### 11.3 — History Queries

The `baton history` command and the library's `History` class provide common queries:

```
Class History:
  static method open(db_path: string) → History:
    # Open database in WAL mode.
    # If db_path does not exist, return an error (don't create implicitly).

  method recent(n: integer, gate: string | null, status: string | null) → list[Verdict]
  method for_artifact(hash: string) → list[Verdict]
  method for_context(hash: string) → list[Verdict]
  method since(datetime) → list[Verdict]
  method stats(gate: string, since: datetime | null) → GateStats


Record GateStats:
  total_runs:            integer
  pass_count:            integer
  fail_count:            integer
  error_count:           integer
  avg_duration_ms:       integer
  validator_fail_rates:  map[string → float]  # per-validator failure rate
  total_cost:            Cost                  # aggregated across all runs
  total_input_tokens:    integer
  total_output_tokens:   integer
```

---

## 12. Integration Patterns

Baton is a gate, not a framework. It integrates at the boundary between "agent produced something" and "something accepts or rejects it."

### 12.1 — CLI (Manual or Scripted)

The simplest integration. A developer or script runs baton directly.

```bash
# Validate a patch file against a task spec
baton check \
  --gate code-review \
  --artifact ./output/patch.diff \
  --context spec=./tasks/issue-42.md

# Returns exit code 0 (pass), 1 (fail), or 2 (error)
# Structured verdict written to stdout as JSON
```

Also useful in CI/CD pipelines:

```yaml
# In a GitHub Action or similar
- name: Validate agent output
  run: |
    baton check --gate code-review \
      --artifact ${{ steps.agent.outputs.patch }} \
      --context spec=./tasks/${{ github.event.issue.number }}.md
```

### 12.2 — Library Call (From an Orchestrator)

For orchestrators like CrewAI, LangGraph, Symphony, or custom scripts, baton exposes a library API:

```
gate = Gate.from_config("baton.toml", "code-review")

verdict = gate.run(
  artifact = Artifact.from_file("./output/patch.diff"),
  context = Context.from_map({
    "spec": "./tasks/issue-42.md"
  })
)

if verdict.status == "fail":
  # Caller decides retry policy: how many attempts, what feedback to pass,
  # whether to escalate, etc. Baton does not manage this loop.
  agent.send("Your output failed validation: " + verdict.feedback)
  new_output = agent.collect()
  # Re-run the gate with the new output
  verdict = gate.run(
    artifact = Artifact.from_string(new_output),
    context = Context.from_map({"spec": "./tasks/issue-42.md"})
  )
else if verdict.status == "error":
  # A validator crashed or was unreachable. Investigate.
  log_error("Validation error: " + verdict.feedback)
```

### 12.3 — Agent-Callable Tool (MCP / Function Calling)

An agent can be instructed to self-validate before submitting work. Baton is exposed as a tool the agent can call:

```json
{
  "name": "baton_check",
  "description": "Validate your output before submitting. Returns PASS, FAIL, or ERROR with feedback.",
  "parameters": {
    "gate": "code-review",
    "artifact_path": "./output/patch.diff",
    "context": {
      "spec": "./tasks/issue-42.md"
    }
  }
}
```

**Caveat:** This is the weakest isolation model. The agent sees the validation feedback in its own context, which means subsequent attempts are not context-isolated from the validator. For stronger isolation, use the orchestrator pattern (12.2) where the caller mediates between agent and gate.

### 12.4 — Git Hook

Pre-commit or pre-push validation of agent-generated code:

```bash
#!/bin/sh
# .git/hooks/pre-push

# Pipe the diff to baton via stdin
git diff --cached | baton check --gate code-review \
  --artifact - \
  --context spec=./SPEC.md

if [ $? -ne 0 ]; then
  echo "baton: validation failed. See verdict above."
  exit 1
fi
```

### 12.5 — Integration Comparison

| Pattern | Isolation | Retry control | Setup effort | Best for |
|---|---|---|---|---|
| CLI | Strong | Manual/scripted | Minimal | Experimentation, CI/CD |
| Library | Strong | Orchestrator | Moderate | Production multi-agent systems |
| Agent tool | Weak | Agent-controlled | Minimal | Single-agent self-validation |
| Git hook | Strong | None (block only) | Minimal | Safety net / last resort |

---

## 13. Testing Strategy

This section provides guidance for implementers on testing baton. The hook-based architecture is designed to make every component testable in isolation.

### 13.1 — Unit Test Targets

Each of these components should have isolated unit tests:

**Config parsing:**
- Valid TOML with all fields → parses correctly.
- Missing required fields → specific error messages.
- Environment variable interpolation: set, unset, with default, empty default, escaped `$${`.
- Invalid validator names (special characters, empty string, duplicates).
- `run_if` expressions: valid, syntax errors, forward references, nonexistent references.
- Version mismatch.
- `blocking` field: omitted (falls back to default), explicit true, explicit false.
- `warn_exit_codes`: valid list, contains 0 (error), empty list, non-integer values.
- Freeform validator with `blocking = true` → config validation warning.

**Prompt resolution:**
- Value with `.md` extension → file lookup in `prompts_dir`, then as literal path.
- Value with `.txt`, `.prompt`, `.j2` extensions → same file lookup.
- Value without extension → inline string.
- File not found → error.
- Empty file → error.
- Non-UTF-8 file → error.
- Multi-line inline TOML string → works correctly.

**Placeholder resolution:**
- All placeholder types resolve correctly.
- Missing optional context → empty string + warning.
- Missing required context → caught in pre-flight.
- Forward reference to validator → caught in validation.
- Unrecognized placeholder → literal string + warning.
- Nested placeholders (e.g., `{context.{name}}`) → treated as literal (not supported).

**Verdict parsing:**
- First line "PASS" → pass.
- First line "PASS — all good" → pass (word boundary after PASS).
- First line "PASSWORD" → does not match (word boundary check rejects).
- First line "PASSING" → does not match (word boundary check rejects).
- First line "FAIL some reason" → fail with evidence.
- First line "WARN minor issue" → warn with evidence.
- Text with reasoning then "PASS" at end → pass.
- Text with both "FAIL" and "PASS", PASS last → pass.
- Text containing "PASSWORD" and then "PASS" → pass (PASSWORD skipped by word boundary, PASS matches).
- Empty text → error.
- No verdict keyword anywhere → error.
- Very long output → evidence truncated.

**`run_if` evaluation:**
- Simple condition: `"lint.status == pass"` with lint passed → true.
- AND: `"lint.status == pass and typecheck.status == pass"` → both must be true.
- OR: `"lint.status == fail or typecheck.status == fail"` → either is true.
- Mixed: `"a.status == pass and b.status == pass or c.status == pass"` → left-to-right, no precedence.
- Referenced validator was skipped → status is "skip".
- Referenced validator not in results (filtered out) → treated as "skip".
- Invalid expression syntax → error (not silent failure).

**Script validator exit codes:**
- Exit 0 → pass.
- Exit 1 (not in warn_exit_codes) → fail.
- Exit 2 (in warn_exit_codes = [2]) → warn with feedback.
- Exit 2 (warn_exit_codes not set) → fail.
- warn_exit_codes = [] → all nonzero are fail.

**Suppression:**
- `--suppress-errors`: error treated as pass, fail still fails.
- `--suppress-warnings`: warn treated as pass (already was, but verify suppressed field).
- `--suppress-all`: error + fail → pass.
- Suppressed results still appear with true status in history array.
- Blocking validator with error + `--suppress-errors` → does not halt.

### 13.2 — Integration Test Fixtures

**Mock provider:** Create a mock HTTP server that responds to `/v1/chat/completions` with configurable responses. Test scenarios:
- Returns "PASS" → validator passes.
- Returns "FAIL missing auth" → validator fails with evidence.
- Returns HTTP 401 → validator errors with auth message.
- Returns HTTP 429 → validator errors with rate limit message (no retry).
- Hangs forever → validator times out.
- Returns empty body → validator errors.
- Returns malformed JSON → validator errors.

**Mock runtime adapter:** Implement a `MockRuntimeAdapter` that returns configurable `SessionResult` objects without launching any actual agent. Test:
- Session completes with "PASS" → validator passes.
- Session completes with no verdict keyword → validator errors.
- Session creation fails → validator errors.
- Session times out → validator errors.
- Cancel is called on SIGINT → session cleaned up.

**Script validator fixtures:** Create small scripts that test specific behaviors:
- `exit 0` → pass.
- `exit 1` with stderr → fail with feedback.
- `exit 2` with warn_exit_codes=[2] → warn with feedback.
- `sleep 999` → times out.
- Non-existent command → error.
- Script that outputs non-UTF-8 bytes → feedback includes replacement characters or hex.

### 13.3 — End-to-End Test Scenarios

These test the full pipeline from config to verdict:

1. **Happy path:** A gate with 3 script validators, all pass. Verdict is pass.
2. **First failure blocks:** A gate with 3 blocking validators, second fails. Third does not run. Verdict is fail at the second validator.
3. **Non-blocking failure:** A gate with a non-blocking validator that fails, then a passing one. Verdict is pass (the failure is recorded in history but doesn't affect the gate verdict).
4. **`--all` mode:** A gate with 3 blocking validators, first and third fail. All three run. Verdict is fail.
5. **`--all` with non-blocking failure:** A gate with a non-blocking validator that fails. In normal mode, verdict is pass. In `--all` mode, verdict is fail. (This tests the dual behavior change of `--all`.)
6. **Conditional skip:** A gate where validator B has `run_if = "A.status == pass"`. A fails. B is skipped.
7. **Warning from script:** A script validator with `warn_exit_codes = [2]`, script exits 2. Gate verdict is pass. Warning recorded in history.
8. **Warning from LLM:** A validator returns WARN. Gate verdict is pass. Warning recorded in history.
9. **Error vs. fail:** A validator crashes (error). Gate verdict is error, not fail.
10. **Signal handling:** Send SIGINT during a long-running script validator. Verify partial verdict is written and temp files are cleaned.
11. **Config validation:** Run `baton validate-config` against both valid and invalid configs. Check all error messages.
12. **History persistence:** Run a gate, then query `baton history`. Verify the verdict appears.
13. **Error suppression:** A gate with `--all --suppress-errors` where one validator errors and one fails. Verdict is fail (not error, because error is suppressed).
14. **Full suppression:** A gate with `--all --suppress-all` where validators error and fail. Verdict is pass.

### 13.4 — Testing Recommendations for Implementers

- **Record LLM responses.** For reproducible tests, record real LLM API responses and replay them in tests. This avoids flaky tests from LLM nondeterminism and avoids API costs in CI.
- **Test error messages.** Baton's error messages are part of the user contract. Assert on specific message formats, not just error presence.
- **Test the CLI as a subprocess.** Spawn `baton check` as a child process in tests. Verify exit codes, stdout JSON, and stderr messages. This catches argument parsing bugs that unit tests miss.
- **Test concurrent access.** If your implementation supports parallel CI, write a test that runs two `baton check` invocations concurrently writing to the same history database. Verify no crashes and no lost verdicts.
- **Fuzz placeholder resolution.** Throw random strings with `{`, `}`, `${`, and nested combinations at the placeholder resolver. It should never crash — only produce warnings for unrecognized patterns.

---

## Appendix A: Prompt Template Library (Starters)

These are initial templates to be refined through empirical testing. They are starting points, not best practices.

### spec-compliance.md

```markdown
+++
description = "Check whether an implementation satisfies its specification"
expects = "verdict"
+++

You are a code reviewer. Given a specification and an implementation,
determine whether the implementation satisfies the specification.

Check ONLY whether spec requirements are met.
Do NOT check style, performance, or suggest alternatives.

Respond with exactly one of:
- PASS — if all requirements are satisfied
- FAIL {requirement} — cite the unmet requirement and explain why

PASS is a valid and expected response. Do not invent requirements.

## Specification

{context.spec.content}

## Implementation

{artifact_content}
```

### adversarial-review.md

```markdown
+++
description = "Hostile review focused on bugs that survive standard testing"
expects = "verdict"
+++

You are a hostile code reviewer trying to find bugs that would
survive standard testing. Focus on:

- Off-by-one errors and boundary conditions
- Unhandled edge cases (empty input, null, overflow)
- Race conditions or state mutations
- Logic errors where code runs without error but produces wrong results

Do NOT flag style issues, naming, or performance.
If you cannot find a concrete bug with a specific reproduction case,
respond PASS.

Respond with exactly one of:
- PASS — if no concrete bugs found
- FAIL {bug description} — with a specific input that triggers the bug
- WARN {concern} — if you see a suspicious pattern but cannot construct
  a concrete failing input

You MUST provide a concrete failing input if you respond FAIL.
Speculation without a reproduction case should be WARN, not FAIL.

## Implementation

{artifact_content}
```

### doc-completeness.md

```markdown
+++
description = "Check structural completeness of a document against its specification"
expects = "verdict"
+++

You are reviewing a document against its specification.
Determine whether the document addresses all required sections
and contains the information specified.

Do NOT evaluate quality, writing style, or correctness of claims.
ONLY check structural completeness against the spec.

Respond with exactly one of:
- PASS — if all specified sections are present and populated
- FAIL {section} — cite the missing or empty section
- WARN {section} — if a section exists but appears incomplete

## Specification

{context.spec.content}

## Document

{artifact_content}
```

---

## Appendix B: Deferred Features and Optimizations

> **Note for agents implementing this spec:** Consider removing this appendix before using this file as implementation context, to prevent premature optimization. Everything below is explicitly out of scope for this version.

These features are architecturally anticipated but not yet specified. The v0.4 design avoids painting itself into a corner for any of them.

### B.1 — Parallel Validator Execution

Some validators (e.g., linting and type checking) have no data dependencies and could run in parallel. This would reduce wall-clock time for gates with many independent validators.

**Why deferred:** Adds complexity to `run_if` evaluation, error reporting order, and `blocking` behavior. The sequential model is correct and simple. Parallel execution is a performance optimization that should be driven by measured bottlenecks.

**Architectural preparation:** Validators are already stateless and communicate only through the `prior_results` map. A parallel executor would partition validators into dependency groups (based on `run_if`) and run each group concurrently.

### B.2 — Composite Gates

A gate that references another gate as a "sub-gate," enabling reuse (e.g., a `full-review` gate that includes `lint-and-type` and `llm-review` as sub-gates).

**Why deferred:** Adds complexity to config validation (circular references), verdict aggregation, and error reporting. Achievable today by defining all validators in a single gate.

### B.3 — Per-Gate Cost Budgets

Stop execution if a cost threshold is exceeded (e.g., "this gate should never cost more than $0.50 per invocation").

**Why deferred:** Requires reliable cost estimation, which depends on provider-specific pricing data. The v0.4 schema captures token counts, which is the prerequisite. Cost budgets can be layered on top.

**Architectural preparation:** The `Cost` record on every `ValidatorResult` ensures token data is always available. A budget check would sum costs after each validator and halt if the threshold is exceeded.

### B.4 — Validator Caching

Skip a validator if the artifact hash (and relevant context hashes) match a recent pass in the history database. Useful in retry loops where only part of the artifact changed.

**Why deferred:** Cache invalidation is hard. A validator might pass on one artifact hash but the external world changed (e.g., new dependencies, updated spec). Requires a cache key strategy and TTL mechanism.

**Architectural preparation:** The `artifact_hash` and `context_hash` fields in the verdict make cache key computation straightforward. The history database already supports querying by these hashes.

### B.5 — Retry Feedback Injection

Allow the caller to pass previous validator feedback into the current gate run (e.g., so the validator can see "I flagged X on the last attempt — check if it was fixed"). Currently forbidden by the stateless validator principle.

**Why deferred:** Breaks context isolation. May be valuable in some workflows, but should be opt-in and explicitly marked as reducing isolation. Could be implemented as a special context item (e.g., `--context _prior_feedback=<path>`).

### B.6 — Human Validator Enhancements

The current human validator is minimal (immediately fail with a prompt). Future enhancements:

- Timeout with configurable duration (fail as error after N minutes).
- Webhook notification (ping a URL when human review is needed).
- Interactive stdin/TTY mode for CLI usage.
- Integration with review tools (e.g., post a GitHub PR comment and wait for approval).

**Architectural preparation:** The human validator returns a `fail` with a `[human-review-requested]` prefix in the feedback. Callers can match on this prefix to route to human review workflows. The validator itself remains stateless.


### B.7 — Context Enhancements

Each context slot maps to a single file. The responsibility of collecting and concatenating context files falls to the user. Multiple named context slots are fully supported (each with its own file), and validators selectively reference only the slots they need via `context_refs`.

**Why deferred:** While single-file-per-slot does pose limitations, it greatly simplifies observability concerns. Invalid context files in a directory could change hashes. How the file is concatenated could affect output quality. Recursive file selection can be bad.

**Possible future direction:** Allow a context slot to accept an ordered list of files (not a directory glob) with explicit ordering, e.g., `--context spec=./overview.md,./api.md,./constraints.md`. This would concatenate in the given order with a configurable separator, preserving determinism and observability while reducing friction. The hash would cover both file contents and their ordering.
