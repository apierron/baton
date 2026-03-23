.PHONY: smoke smoke-claude smoke-api test lint

# Run smoke tests (defaults to claude-code runtime)
smoke:
	cargo test --test smoke -- --ignored --nocapture

# Run smoke tests with Claude Code runtime (explicit)
smoke-claude:
	BATON_SMOKE_RUNTIME=claude-code cargo test --test smoke -- --ignored --nocapture

# Run smoke tests with an API runtime
# Requires BATON_SMOKE_BASE_URL, BATON_SMOKE_API_KEY_ENV, and the actual API key env var to be set
smoke-api:
	BATON_SMOKE_RUNTIME=api cargo test --test smoke -- --ignored --nocapture

# Standard targets
test:
	cargo test

lint:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
