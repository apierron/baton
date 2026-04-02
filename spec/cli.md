# Baton CLI Specification

Baton is a composable validation gate for AI agent outputs. It accepts input files, runs validators (script/LLM/human) against them through a gate orchestration pipeline, produces structured results, and persists invocation history in SQLite.

## Composition model

Baton uses a three-layer composition model: **sources → validators → gates**.

- **Sources** are named file sets. They give a name to a directory, a single file, or an explicit list of files.
- **Validators** are stateless functions. Each declares what it does (type, command/prompt) and what files it needs (input declarations). Validators know nothing about orchestration.
- **Gates** are the orchestration layer. A gate lists which validators to run, in what order, with what sequencing rules (`blocking`, `run_if`). The same validator can appear in multiple gates with different orchestration settings.

## Config discovery

Baton searches for `baton.toml` by walking up the directory tree from the current directory. Traversal stops at `.git` boundaries to prevent using a config from a parent repository. Use `--config <path>` to skip discovery and use an explicit path.

---

## Commands

### `baton check`

Run validators against input files. This is the core command.

```
baton check [FILES...] [OPTIONS]
```

Positional arguments are input files and directories (directories are walked recursively by default). Files from all sources (positional args, `--diff`, `--files`, source declarations) are merged into a single input pool.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--config <path>` | path | discovered | Path to baton.toml |
| `--only <selectors...>` | string list | all | Only run matching gates/validators |
| `--skip <selectors...>` | string list | none | Skip matching gates/validators |
| `--diff <refspec>` | string | — | Add git-changed files to the input pool |
| `--files <path\|->` | string | — | Read newline-separated file paths from a file or stdin (`-` for stdin) |
| `--timeout <seconds>` | integer | — | Override default timeout for all validators |
| `--format <format>` | string | `json` | Output format: `json`, `human`, or `summary` |
| `--dry-run` | flag | — | Print invocation plan and exit without running validators |
| `--no-log` | flag | — | Don't write to history database or log files |
| `-v, --verbose` | flag | — | Print each validator's result as it completes |
| `--suppress-warnings` | flag | — | Treat warn statuses as pass |
| `--suppress-errors` | flag | — | Treat error statuses as pass |
| `--suppress-all` | flag | — | Suppress warnings, errors, and failures |
| `--no-recursive` | flag | — | Disable recursive directory walking for positional args |

**Selectors** (used in `--only` and `--skip`) accept:
- Gate name: `review`
- Validator name: `lint`
- Dot-path: `review.lint` (specific validator within a specific gate)
- Tag: `@fast`

`--skip` is applied after `--only` — it removes from whatever set `--only` selected.

**Dry run** prints which validators would run, with what inputs, which would be skipped and why, and any `run_if` expressions. No validators execute, no verdict is produced, nothing is written to stdout. Exits 0.

### `baton init`

Initialize a new baton project.

```
baton init [OPTIONS]
```

Creates `baton.toml`, `.baton/` directory structure (logs, tmp), and optionally starter prompt templates.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--minimal` | flag | — | Only create baton.toml and .baton/ directory (no prompt templates) |
| `--prompts-only` | flag | — | Only create the prompts/ directory with starter templates |
| `--profile <name>` | string | `generic` | Language profile: `rust`, `python`, or `generic` |

When stdin is a TTY and no flags are provided, enters interactive mode with prompts for language, validators, and templates. When not a TTY, uses defaults (generic profile, prompts included).

Exits 1 if `baton.toml` already exists. Exits 1 for unknown profile names.

### `baton add`

Add a validator to baton.toml, interactively or via flags.

```
baton add [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--name <name>` | string | — | Validator name |
| `--type <type>` | string | — | Validator type: `script`, `llm`, or `human` |
| `--command <cmd>` | string | — | Script command |
| `--prompt <text>` | string | — | LLM/human prompt text |
| `--runtime <name>` | string | — | Runtime name for LLM validators |
| `--model <name>` | string | — | Model override for LLM validators |
| `--gate <name>` | string | — | Add to this gate (existing or new) |
| `--blocking <bool>` | bool | — | Whether the validator is blocking in the gate |
| `--tags <tags>` | comma-separated | — | Tags to apply |
| `--input <glob>` | string | — | Input glob pattern |
| `--timeout <seconds>` | integer | — | Timeout in seconds |
| `--from <source>` | string | — | Import from file, URL, or registry |
| `--config <path>` | path | discovered | Path to baton.toml |
| `--dry-run` | flag | — | Preview changes without writing |
| `-y, --yes` | flag | — | Skip confirmation prompt |

### `baton list`

List available gates and validators.

```
baton list [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--gate <name>` | string | — | Show validators for a specific gate |
| `--config <path>` | path | discovered | Path to baton.toml |

Without `--gate`, lists all gates with names, descriptions, and validator counts. With `--gate`, shows which validators the gate references and their `blocking`/`run_if` settings.

Gates are listed in alphabetical order (BTreeMap key order).

### `baton history`

Query invocation history from the SQLite database.

```
baton history [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--gate <name>` | string | — | Filter by gate name |
| `--status <status>` | string | — | Filter by status |
| `--file <path>` | string | — | Search validator runs by file path |
| `--hash <sha256>` | string | — | Search validator runs by content hash |
| `--invocation <id>` | string | — | Show detail for a specific invocation |
| `--limit <n>` | integer | `20` | Number of results |
| `--config <path>` | path | discovered | Path to baton.toml |

Without `--file`, `--hash`, or `--invocation`, shows recent invocations. Prints "No verdicts found." when no results match.

### `baton doctor`

Run comprehensive health checks on the baton installation and project.

```
baton doctor [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--config <path>` | path | discovered | Path to baton.toml |
| `--offline` | flag | — | Skip checks that require network access (runtime health checks) |

Checks are grouped into six sections:

1. **Installation** — baton version and install method
2. **Configuration** — config discovery, parsing, and validation
3. **Project Structure** — directories (prompts, logs, tmp) and history database
4. **Prompt Templates** — resolves all LLM validator prompt file references
5. **Environment** — checks `api_key_env` variables for each runtime
6. **Runtimes** — health checks all configured runtimes (skipped with `--offline`)

Each check gets a status prefix: `[ok]`, `[warn]`, `[fail]`, or `[skip]`. A summary line is printed at the end. All output goes to stderr.

Exits 0 if no checks fail. Exits 1 if any check fails.

### `baton clean`

Remove stale temporary files from the configured tmp directory.

```
baton clean [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--dry-run` | flag | — | Show what would be cleaned without deleting |
| `--config <path>` | path | discovered | Path to baton.toml |

Files older than 1 hour are considered stale. Always exits 0.

### `baton version`

Print version information.

```
baton version [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--config <path>` | path | discovered | Path to baton.toml |

Prints baton version (from `Cargo.toml`), spec version, and config file location (or "not found"). Always exits 0.

### `baton update`

Update baton to the latest version.

```
baton update [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--version <version>` | string | latest | Install a specific version (e.g. `0.4.2` or `v0.4.2`) |
| `-y, --yes` | flag | — | Skip confirmation prompt |

For cargo and homebrew installations, prints the appropriate package-manager update command and exits 1 (does not modify anything). For binary installations, downloads from GitHub releases and replaces the current executable. Prints "Already up to date" and exits 0 if versions match.

### `baton uninstall`

Uninstall baton from the system.

```
baton uninstall [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--all` | flag | — | Remove all baton installations, not just the one in PATH |
| `-y, --yes` | flag | — | Skip confirmation prompt |

The currently running binary is always targeted. With `--all`, also searches `~/.local/bin/baton`, `$CARGO_HOME/bin/baton`, and homebrew locations (deduplicating by canonical path). Cargo installations attempt `cargo uninstall baton` first. The current executable is deleted last.

Exits 0 on full success. Exits 1 if any removal fails.

---

## Configuration (baton.toml)

### Version

```toml
version = "0.7"
```

The `version` field must be `"0.4"`, `"0.5"`, `"0.6"`, or `"0.7"`.

### Defaults

```toml
[defaults]
timeout_seconds = 300       # Default timeout for all validators
blocking = true             # Default blocking behavior for gate refs
prompts_dir = "./prompts"   # Directory for prompt templates
log_dir = "./.baton/logs"   # Log file directory
history_db = "./.baton/history.db"  # SQLite history database
tmp_dir = "./.baton/tmp"    # Temporary file directory
```

All values shown are the built-in defaults. Relative paths are resolved against the directory containing `baton.toml`.

### Runtimes

```toml
[runtimes.my-runtime]
type = "api"                    # Runtime type
base_url = "https://api.example.com"  # API endpoint (env vars resolved at parse time)
api_key_env = "API_KEY"         # Name of env var holding the API key
default_model = "my-model"      # Default model (optional)
sandbox = true                  # Sandbox mode (default: true)
timeout_seconds = 600           # Runtime timeout (default: 600)
max_iterations = 30             # Max session iterations (default: 30)
```

For `type = "api"` runtimes, trailing slashes on `base_url` are stripped. Environment variable references in `base_url` (e.g., `${VAR}`) are resolved at parse time.

### Sources

```toml
# Directory source — walk a directory with include/exclude globs
[sources.code]
root = "./src"
include = ["**/*.rs"]       # Default: ["**/*"]
exclude = ["**/test_*"]     # Default: []

# Single file source
[sources.readme]
path = "./README.md"

# Explicit file list
[sources.configs]
files = ["config.toml", "settings.json"]
```

Only one of `root`, `path`, or `files` may be set per source. Source names must match `[a-zA-Z0-9_-]+`.

### Validators

Validators are defined as top-level entries under `[validators]`.

```toml
[validators.lint]
type = "script"
command = "cargo clippy --all-targets -- -D warnings"
tags = ["fast"]
timeout_seconds = 60
warn_exit_codes = [2]       # Exit codes that produce warnings instead of failures
working_dir = "./src"       # Working directory for the command
env = { RUST_LOG = "debug" }  # Extra environment variables

[validators.review]
type = "llm"
runtime = "my-runtime"      # Required for LLM validators (string or list for fallback)
prompt = "Review this code: {file}"
mode = "query"              # "query" (default) or "session"
model = "my-model"          # Overrides runtime's default_model
temperature = 0.0           # Default: 0.0
response_format = "verdict" # "verdict" (default) or "freeform"
max_tokens = 4096
system_prompt = "You are a code reviewer."
tags = ["slow"]

[validators.check]
type = "human"
prompt = "Please review: {file}"
```

**Validator types:**

| Type | Required fields | Behavior |
|------|----------------|----------|
| `script` | `command` | Runs a shell command. Exit 0 = pass, non-zero = fail. Codes in `warn_exit_codes` produce warnings. |
| `llm` | `prompt`, `runtime` | Sends prompt to an LLM via the specified runtime. Supports query mode (one-shot) and session mode (multi-step). |
| `human` | `prompt` | Always fails with `[human-review-requested]` prefix. Signals that a human needs to take action. |

**LLM modes:**

- `query` (default): One-shot completion. Builds a request and calls the runtime's completion endpoint. Falls back through the runtime list if a runtime is unreachable or doesn't support completions.
- `session`: Multi-step interactive session. Creates a session, polls until completion, collects results. API-type runtimes are skipped (session requires interactive capability). Falls back through non-API runtimes.

**Response formats:**

- `verdict` (default): LLM must respond with a PASS/FAIL/WARN verdict.
- `freeform`: LLM response is treated as advisory feedback, always producing a warn status. Note: `freeform` with `blocking = true` has no effect (warn never triggers blocking).

### Gates

```toml
[gates.review]
description = "Code review gate"
validators = [
    { ref = "lint", blocking = true },
    { ref = "review", blocking = false, run_if = "lint.status == pass" },
]
```

Gates reference validators by name with optional orchestration overrides:

| Field | Default | Description |
|-------|---------|-------------|
| `ref` | required | Name of a validator defined in `[validators]` |
| `blocking` | from `[defaults]` | If true and this validator fails, stop the pipeline |
| `run_if` | — | Conditional expression (only run if condition is met) |
| `timeout_seconds` | from validator/defaults | Timeout override for this gate reference |

### Environment variable interpolation

Config strings support `${VAR}` interpolation, resolved at parse time:

- `${VAR}` — substitutes the value of `VAR`; errors if unset
- `${VAR:-default}` — uses `default` if `VAR` is unset (empty values are not considered unset)
- `$${` — literal `${` (escape sequence)

---

## Validator types

### Script validators

Run a shell command as a subprocess (`sh -c` on Unix, `cmd /C` on Windows). Placeholders in the command string are resolved before execution.

- **Exit 0**: pass (no feedback)
- **Exit N in `warn_exit_codes`**: warn (stdout+stderr as feedback)
- **Exit N not in `warn_exit_codes`**: fail (stdout+stderr as feedback, or `[baton] Script exited with code N (no output)` if empty)
- **Command not found / permission denied / empty command**: error

Both stdout and stderr are captured and combined in feedback.

### LLM validators

Send a prompt to an LLM through a configured runtime. The prompt is resolved with placeholders before sending.

**Model resolution chain**: validator `model` → runtime `default_model` → `"default"`.

**Runtime fallback**: runtimes are tried in order. If a runtime is unreachable, doesn't support the operation, or fails to create an adapter, the next runtime is tried. Once a runtime returns a result (even error/fail), that result is final. If all runtimes are exhausted, returns error.

**Cost tracking**: token usage and estimated cost from the runtime are propagated to the validator result.

### Human validators

Always return fail with feedback `[human-review-requested] {rendered_prompt}`. The prompt is resolved with placeholders so the reviewer gets full context.

---

## Gates and orchestration

### Blocking

When a validator has `blocking = true` and its effective status (after suppression) is fail or error, the pipeline stops immediately. No subsequent validators execute. The verdict names the blocking validator in `failed_at`.

When `blocking = false`, failures are recorded but execution continues to the next validator.

### run_if

Conditional expressions evaluated before dispatching a validator. If the expression evaluates to false, the validator is recorded as skip.

Syntax: `<name>.status == <value>` where `<value>` is one of `pass`, `fail`, `warn`, `error`, `skip`.

Expressions support `and` / `or` operators (left-to-right, no precedence, no short-circuit):

```
lint.status == pass and format.status == pass
lint.status == fail or review.status == fail
```

A reference to a nonexistent validator (e.g., filtered out by `--skip`) is treated as `skip`.

### Status suppression

Suppression converts statuses to pass before the blocking check and final status computation:

- `--suppress-warnings`: warn → pass
- `--suppress-errors`: error → pass
- `--suppress-all`: warn, error, fail → pass

The verdict history always records the true (unsuppressed) status.

### Final status computation

In normal mode (default), if the pipeline completes without a blocking failure, the verdict is pass. Non-blocking failures are advisory — they appear in history but don't fail the gate.

Priority when computing aggregate status: error > fail > pass. Warn is treated as pass. Skip is ignored. Empty results produce pass.

---

## Input declarations

Validators declare what files they need via the `input` field. The dispatch planner turns the file pool into invocations based on these declarations.

### No input

When `input` is absent, the validator runs once with no files. Useful for project-level checks (e.g., `cargo test`).

### Per-file

When `input` is a glob string, the validator runs once per matching file.

```toml
[validators.lint]
type = "script"
command = "rustfmt --check {file.path}"
input = "**/*.rs"
```

### Batch

When `input` is an object with `match` and `collect = true`, all matching files are passed at once.

```toml
[validators.review]
type = "llm"
prompt = "Review these files: {input}"
input = { match = "**/*.rs", collect = true }
```

### Named inputs

When `input` has named sub-keys, each is a separate input slot with `match`, `path`, or `key`.

```toml
[validators.spec-check]
type = "llm"
prompt = "Check {input.code} against {input.spec}"
input.code = { match = "src/**/*.rs", key = "{stem}" }
input.spec = { match = "spec/**/*.md", key = "{stem}" }
```

Key expressions for grouping: `{stem}`, `{name}`, `{parent}`, `{relative:prefix/}`, `{regex:pattern}`.

---

## Placeholders

Placeholders use `{...}` syntax and are resolved at execution time against the invocation's input files and prior validator results.

### Per-file placeholders

Available when a validator operates in per-file mode:

| Placeholder | Resolves to |
|-------------|-------------|
| `{file}` | File content (UTF-8) |
| `{file.content}` | File content (alias for `{file}`) |
| `{file.path}` | Absolute file path |
| `{file.dir}` | Parent directory |
| `{file.name}` | Filename with extension |
| `{file.stem}` | Filename without extension |
| `{file.ext}` | Extension without dot |

### Batch placeholders

Available in batch mode:

| Placeholder | Resolves to |
|-------------|-------------|
| `{input}` | Concatenated content of all matched files |
| `{input.paths}` | Space-separated absolute paths |

### Named input placeholders

Available when a validator has named input slots:

| Placeholder | Resolves to |
|-------------|-------------|
| `{input.<name>}` | File content |
| `{input.<name>.content}` | File content (explicit) |
| `{input.<name>.path}` | Absolute path |
| `{input.<name>.name}` | Filename |
| `{input.<name>.stem}` | Filename stem |
| `{input.<name>.paths}` | Space-separated paths (multiple files) |

### Verdict placeholders

Reference prior validator results by name:

| Placeholder | Resolves to |
|-------------|-------------|
| `{verdict.<name>.status}` | Status string: `pass`, `fail`, `warn`, `error`, `skip` |
| `{verdict.<name>.feedback}` | Feedback text (empty if none) |

Missing validators default to status `skip` and empty feedback (no error produced).

### Resolution behavior

- Unrecognized placeholders are left as literal text (including braces) with a warning
- Unclosed braces are left as literal characters
- Resolution failures produce warnings and empty strings (not errors)

---

## Output formats

Baton supports three output formats via `--format`:

### `json` (default)

Pretty-printed JSON to **stdout**. Machine-parseable verdict including status, gate name, validator history, timing, warnings, and suppressed statuses.

```json
{
  "status": "pass",
  "gate": "review",
  "failed_at": null,
  "feedback": null,
  "duration_ms": 1234,
  "timestamp": "2025-01-01T00:00:00Z",
  "warnings": [],
  "suppressed": [],
  "history": [
    {
      "name": "lint",
      "status": "pass",
      "feedback": null,
      "duration_ms": 500,
      "cost": null
    }
  ]
}
```

### `human`

Multi-line human-readable output to **stderr** with status icons:

```
  ✓ lint (500ms)
  ✗ review (700ms)
    missing semicolon on line 42
  VERDICT: FAIL (failed at: review)
```

Status icons: `✓` pass, `✗` fail, `!` warn, `—` skip, `E` error.

Feedback is shown for non-pass validators (up to 5 lines, indented). Pass feedback is suppressed.

### `summary`

One-line summary to **stderr**:

- Pass: `PASS`
- Fail: `FAIL at <validator>: <first line of feedback>`
- Error: `ERROR at <validator>: <first line of feedback>`

An unrecognized `--format` value falls back to JSON with a warning on stderr.

---

## Filtering

The `--only` and `--skip` flags accept selectors to control which gates and validators run.

### Selector syntax

| Selector | Matches |
|----------|---------|
| `name` | Gates or validators with this name |
| `gate.validator` | A specific validator within a specific gate |
| `@tag` | Validators with the given tag |

### Gate-level filtering

- `--only` with a gate name includes that gate. With a validator name, includes gates containing that validator. With `@tag`, includes gates containing a validator with that tag.
- `--skip` with a gate name excludes the entire gate. Tags and validator names pass through to the validator-level filter.

### Validator-level filtering

Within included gates, `--only` and `--skip` filter individual validators by name, dot-path, or tag.

### Interaction

`--skip` is applied after `--only`. The final set is: (validators selected by `--only`) minus (validators matched by `--skip`).

Filtered validators are recorded as skip in the verdict history.

---

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success or passing verdict |
| `1` | User-recoverable error or failing verdict |
| `2` | Infrastructure/config error |
