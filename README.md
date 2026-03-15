# Baton :magic_wand:

[![CI](https://github.com/apierron/baton/actions/workflows/ci.yml/badge.svg)](https://github.com/apierron/baton/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/apierron/baton/branch/master/graph/badge.svg)](https://codecov.io/gh/apierron/baton)

## A tool to predictably validate sloppy work

Baton runs a sequence of user-defined checks to validate input. These validators are scripts, LLM queries, or human approvals. Baton works through your requirements in order and returns a structured verdict: **pass** or **fail**.

Running your validation step explicitly makes sure no one forgets to check their work. It can also help maintain guidelines or guardrails that aren't always apparent.

Like in a relay race, you're only done after you ***pass the baton***.

## Design Principles

- **User-defined.** You describe what "valid" means for your domain. It all runs through a single config file, baton.toml.
- **Context-isolated.** Baton only sees what you explicitly provide. This makes validation a stateless function: `artifact + context = verdict`.
- **Observable.** Every step produces structured output. Verdict history is queryable and persisted locally in SQLite. This also allows for previous verification runs to optionally be used as context.

## Installation

### One-liner (preferred method)

```bash
curl -fsSL https://raw.githubusercontent.com/apierron/baton/master/install.sh | bash
```

This installs to `~/.local/bin` by default. Set `BATON_INSTALL_DIR` to customize.

<details>
<summary> Other installation methods (Homebrew, Cargo, binaries) </summary>

### Homebrew (macOS and Linux)

```bash
brew install apierron/tap/baton
```

### Cargo

```bash
cargo install --git https://github.com/apierron/baton.git
```

### Prebuilt binaries

Download builds directly from [GitHub Releases](https://github.com/apierron/baton/releases). Builds are available for:

| Target | Format |
| ------ | ------ |
| `x86_64-unknown-linux-gnu` | `.tar.gz` |
| `aarch64-unknown-linux-gnu` | `.tar.gz` |
| `x86_64-apple-darwin` | `.tar.gz` |
| `aarch64-apple-darwin` | `.tar.gz` |
| `x86_64-pc-windows-msvc` | `.zip` |
| `aarch64-pc-windows-msvc` | `.zip` |

</details>

### Uninstall

```bash
baton uninstall
```

<details>
<summary> Details </summary>

The `uninstall` command will remove the installation it is called from. Run with `--all` to look for all baton installations (Cargo, Homebrew, etc.), not just the one in path. Use `-y` to skip confirmation if you like to live dangerously.

Alternatively, you can use the uninstall script:

```bash
curl -fsSL https://raw.githubusercontent.com/apierron/baton/master/uninstall.sh | bash
```

</details>

### Updating

```bash
baton update
```

<details>
<summary> Details </summary>

To migrate to a specific version, use the `--version` flag:

```bash
baton update --version "0.4.2"
```

Note that if you used a package manager (Homebrew or Cargo) in a previous step, you should use their tooling instead.

</details>

## Quick Start

### 1. Initialize a project

```bash
baton init
```

This creates a `baton.toml` config, a `.baton/` directory for history and logs, and a `prompts/` directory with starter prompt templates. Run with `--minimal` to initialize without example prompts.

### 2. Define a gate

Edit `baton.toml`:

```toml
version = "0.4"

[gates.code-review]
description = "Validates a code patch against a task spec"

  [gates.code-review.context.spec]
  description = "The task specification"
  required = true

  [[gates.code-review.validators]]
  name = "lint"
  type = "script"
  command = "ruff check {artifact}"

  [[gates.code-review.validators]]
  name = "tests"
  type = "script"
  command = "pytest --tb=short"
  blocking = true
```

### 3. Run a gate

```bash
baton check --gate code-review --artifact ./output.py --context spec=./task.md
```

Pipe from stdin:

```bash
cat output.py | baton check --gate code-review --artifact - --context spec=./task.md
```

### 4. View results

The verdict is printed as JSON by default:

```json
{
  "status": "pass",
  "gate": "code-review",
  "artifact_hash": "sha256:...",
  "context_hash": "sha256:...",
  "duration_ms": 1234,
  "warnings": [],
  "history": [
    { "name": "lint", "status": "pass", "duration_ms": 450 },
    { "name": "tests", "status": "pass", "duration_ms": 784 }
  ]
}
```

Use `--format human` for a readable summary or `--format summary` for a one-line result.

## CLI Reference

```text
baton check            Run a gate against an artifact
baton init             Scaffold a new baton project
baton list             List gates and validators in a config
baton history          Query verdict history from the SQLite database
baton validate-config  Check a baton.toml for errors and warnings
baton check-provider   Check provider connectivity and model availability
baton check-runtime    Check runtime connectivity and health
baton clean            Remove temporary files from .baton/tmp/
baton update           Update baton to the latest version
baton uninstall        Uninstall baton from this system
baton version          Print version information
```

### Key flags for `baton check`

| Flag | Description |
| ---- | ----------- |
| `--gate <name>` | Gate to run (required) |
| `--artifact <path>` | Path to artifact, or `-` for stdin (required) |
| `--context <name>=<path>` | Context item (repeatable) |
| `--config <path>` | Path to `baton.toml` (default: auto-discover) |
| `--format <json\|human\|summary>` | Output format |
| `--dry-run` | Print validators that would run and exit |
| `--all` | Run all validators even if a blocking one fails |
| `--only <names>` | Run only named validators (comma-separated) |
| `--skip <names>` | Skip named validators (comma-separated) |
| `--tags <tags>` | Run only validators with these tags |
| `--timeout <seconds>` | Override default timeout for all validators |
| `--no-log` | Don't write to history database |
| `--verbose` | Print each validator's result as it completes |
| `--suppress-warnings` | Treat warn statuses as pass |
| `--suppress-errors` | Treat error statuses as pass |

## Validator Types

### Script

Runs a shell command. Exit code 0 = pass, nonzero = fail. An optional `warn_exit_codes` list maps specific exit codes to `warn`. Stdout/stderr is captured as feedback.

```toml
[[gates.my-gate.validators]]
name = "lint"
type = "script"
command = "ruff check {artifact}"
warn_exit_codes = [2]
```

### LLM

Invokes a language model for validation in one of two modes:

- **completion** — Sends the artifact and a prompt template to a model. The response is parsed for a structured verdict keyword (PASS/FAIL/WARN).
- **session** — Launches a multi-turn agent session via a runtime adapter. The agent can use tools, read files, and produce a verdict grounded in observation.

```toml
[[gates.my-gate.validators]]
name = "spec-check"
type = "llm"
mode = "completion"
prompt = "spec-compliance"
provider = "default"
model = "claude-haiku"
```

### Human

Halts the pipeline and reports a failure with a human-review prompt as feedback. Baton does not block waiting for input — it fails with a clear signal that human review was requested.

```toml
[[gates.my-gate.validators]]
name = "human-review"
type = "human"
prompt = "Review this artifact for correctness"
blocking = false
```

## Configuration

Baton uses TOML for configuration. One file can define multiple named gates, shared defaults, provider configuration, and runtime settings.

Config discovery walks upward from the current directory looking for `baton.toml`, stopping at `.git` boundaries.

Environment variables can be interpolated in any string value:

```toml
api_base = "${CUSTOM_API_BASE:-https://api.anthropic.com}"
api_key_env = "ANTHROPIC_API_KEY"
```

## Prompt Templates

Prompt templates use a `+++`-delimited TOML frontmatter format:

```text
+++
name = "spec-compliance"
description = "Check artifact against a specification"
expects = "verdict"
+++

Review the following artifact against the provided specification.

**Artifact:**
{artifact_content}

**Specification:**
{context.spec.content}

Respond with PASS if the artifact meets the specification, or FAIL with an explanation.
```

Available placeholders: `{artifact}`, `{artifact_dir}`, `{artifact_content}`, `{context.<name>}`, `{context.<name>.content}`, `{verdict.<name>.status}`, `{verdict.<name>.feedback}`.

Three starter templates are included with `baton init`: spec-compliance, adversarial-review, and doc-completeness.

## Verdict History

Baton persists every verdict to a local SQLite database (`.baton/history.db`). Query it:

```bash
# Last 10 verdicts
baton history --limit 10

# Filter by gate
baton history --gate code-review

# Filter by status
baton history --status fail
```

## Implementation Status

### Implemented

- Core types: Artifact (with lazy content/hash loading), Context, Verdict, ValidatorResult, Cost
- Configuration parsing and validation (`baton.toml`, env var interpolation, config discovery)
- Prompt template parsing (TOML frontmatter, placeholder resolution)
- Verdict parsing (word-boundary-aware keyword matching for PASS/FAIL/WARN)
- Script validators (command execution, exit code mapping, stdout/stderr capture)
- LLM completion validators (HTTP POST to OpenAI-compatible `/v1/chat/completions` endpoints)
- LLM session validators (runtime adapter interface with OpenHands implementation)
- Human validators (fail with `[human-review-requested]` feedback)
- Gate execution pipeline (sequential validators, blocking logic, `run_if` conditionals, `--all` mode, status suppression, `--only`/`--skip`/`--tags` filtering)
- Full CLI (check, init, list, history, validate-config, check-provider, check-runtime, clean, version)
- SQLite verdict history (WAL mode, query by gate/status/artifact hash)
- Dry-run mode
- Stdin artifact support (piped input via temp file)
- Output formats: JSON, human-readable, summary
- CI/CD: GitHub Actions for lint, test (Linux/macOS/Windows), cross-platform release builds
- Distribution: Homebrew tap, shell installer, prebuilt binaries

### Not Yet Implemented

- **Timeout enforcement** — SIGTERM/SIGKILL on script validators exceeding timeout
- **Signal handling** — Graceful shutdown on SIGINT/SIGTERM with temp file cleanup
- **Log file writing** — JSON log entries to `<log_dir>/<date>/` (only SQLite history is implemented)
- **TTY auto-detection** — `--format` should default to `human` when stdout is a TTY

## Testing

```bash
cargo test
```

```bash
cargo clippy --all-targets -- -D warnings
```

Coverage is tracked automatically via [Codecov](https://codecov.io/gh/apierron/baton).

## Contributing

Contributions are welcome! Areas where help is especially appreciated:

- **Unimplemented features** - Anything from the list above
- **Additional runtime adapters** — Beyond OpenHands (e.g., Claude Code, Codex, etc.)
- **Additional validator types or prompt templates** — Community-shared validators are always helpful
- **Bugs** - Squash them all

To get started:

1. Fork the repository
2. Create a feature branch
3. Run `cargo test` and `cargo clippy --all-targets -- -D warnings` before submitting
4. Open a pull request

## License

MIT
