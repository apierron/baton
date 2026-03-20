# Baton :magic_wand:

[![CI](https://github.com/apierron/baton/actions/workflows/ci.yml/badge.svg)](https://github.com/apierron/baton/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/apierron/baton/branch/master/graph/badge.svg)](https://codecov.io/gh/apierron/baton)

## A tool to predictably validate sloppy work

Baton runs a sequence of user-defined checks to validate input. These validators are scripts, LLM queries, or human approvals. Baton works through your requirements in order and returns a structured verdict: **pass** or **fail**.

Running your validation step explicitly makes sure no one forgets to check their work. It can also help maintain guidelines or guardrails that aren't always apparent.

Like in a relay race, you're only done after you ***pass the baton***.

## Design Principles

- **User-defined.** You describe what "valid" means for your domain. It all runs through a single config file, baton.toml.
- **Input-driven.** Baton collects input files from CLI args, git diffs, or source declarations, then dispatches them to validators based on pattern matching.
- **Observable.** Every step produces structured output. Invocation history is queryable and persisted locally in SQLite.

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

### 2. Define validators and gates

Edit `baton.toml`:

```toml
version = "0.5"

[validators.lint]
type = "script"
command = "ruff check {file.path}"
input = "*.py"

[validators.tests]
type = "script"
command = "pytest --tb=short"

[gates.code-review]
description = "Validates code changes"
validators = [
  { ref = "lint" },
  { ref = "tests", blocking = true },
]
```

### 3. Run validators

```bash
baton check ./output.py
```

Use `--diff` to validate changed files:

```bash
baton check --diff HEAD~1
```

Or pipe a file list:

```bash
git diff --name-only HEAD~1 | baton check --files -
```

### 4. View results

The result is printed as JSON by default:

```json
{
  "gate_results": [
    {
      "gate": "code-review",
      "status": "pass",
      "duration_ms": 1234,
      "validator_results": [
        { "name": "lint", "status": "pass", "duration_ms": 450 },
        { "name": "tests", "status": "pass", "duration_ms": 784 }
      ]
    }
  ]
}
```

Use `--format human` for a readable summary or `--format summary` for a one-line result.

## CLI Reference

```text
baton check            Run validators against input files
baton init             Scaffold a new baton project
baton list             List gates and validators in a config
baton history          Query invocation history from the SQLite database
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
| `<files...>` | Positional args: input files/directories (dirs walked recursively) |
| `--diff <refspec>` | Add git-changed files to the input pool |
| `--files <path\|->` | Read newline-separated file paths from a file or stdin |
| `--only <selector>` | Run only matching gates/validators (`gate`, `gate.validator`, `@tag`) |
| `--skip <selector>` | Skip matching gates/validators (same syntax as `--only`) |
| `--config <path>` | Path to `baton.toml` (default: auto-discover) |
| `--format <json\|human\|summary>` | Output format |
| `--dry-run` | Print invocation plan and exit |
| `--timeout <seconds>` | Override default timeout for all validators |
| `--no-log` | Don't write to history database |
| `--verbose` | Print each validator's result as it completes |
| `--suppress-warnings` | Treat warn statuses as pass |
| `--suppress-errors` | Treat error statuses as pass |

## Validator Types

### Script

Runs a shell command. Exit code 0 = pass, nonzero = fail. An optional `warn_exit_codes` list maps specific exit codes to `warn`. Stdout/stderr is captured as feedback.

```toml
[validators.lint]
type = "script"
command = "ruff check {file.path}"
input = "*.py"
warn_exit_codes = [2]
```

### LLM

Invokes a language model for validation in one of two modes:

- **completion** — Sends input files and a prompt template to a model. The response is parsed for a structured verdict keyword (PASS/FAIL/WARN).
- **session** — Launches a multi-turn agent session via a runtime adapter. The agent can use tools, read files, and produce a verdict grounded in observation.

```toml
[validators.spec-check]
type = "llm"
mode = "completion"
prompt = "spec-compliance"
provider = "default"
model = "claude-haiku"
input = { code = "*.py", spec = { path = "spec.md" } }
```

### Human

Halts the pipeline and reports a failure with a human-review prompt as feedback. Baton does not block waiting for input — it fails with a clear signal that human review was requested.

```toml
[validators.human-review]
type = "human"
prompt = "Review this code for correctness"
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
description = "Check code against a specification"
expects = "verdict"
+++

Review the following code against the provided specification.

**Code:**
{input.code.content}

**Specification:**
{input.spec.content}

Respond with PASS if the code meets the specification, or FAIL with an explanation.
```

Available placeholders:
- Per-file: `{file}` (content), `{file.path}`, `{file.dir}`, `{file.name}`, `{file.stem}`, `{file.ext}`, `{file.content}` (alias for `{file}`)
- Batch: `{input}`, `{input.paths}`
- Named: `{input.<name>}`, `{input.<name>.path}`, `{input.<name>.name}`, `{input.<name>.content}`
- Verdict: `{verdict.<name>.status}`, `{verdict.<name>.feedback}`

Three starter templates are included with `baton init`: spec-compliance, adversarial-review, and doc-completeness.

## Invocation History

Baton persists every invocation to a local SQLite database (`.baton/history.db`). Query it:

```bash
# Last 10 invocations
baton history --limit 10

# Filter by file
baton history --file src/main.rs

# Filter by content hash
baton history --hash sha256:abc123

# Detail for a specific invocation
baton history --invocation <id>
```

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
