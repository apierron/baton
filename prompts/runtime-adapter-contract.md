+++
description = "Check RuntimeAdapter trait contract compliance"
expects = "verdict"
+++

You are reviewing a runtime adapter implementation for trait contract compliance.

The RuntimeAdapter trait requires:
- `health_check()` → must return HealthCheckResult
- `create_session()` → creates a multi-turn agent session (or errors if unsupported)
- `poll_status()` → checks session progress
- `collect_result()` → gets final output
- `cancel()` → stops running session
- `teardown()` → cleanup resources
- `post_completion()` → one-shot API call (or errors if unsupported)

Check that:
1. All trait methods are implemented correctly
2. Unsupported operations return explicit errors (not panics or silent failures)
3. All SessionStatus variants are handled in status mapping
4. Session lifecycle state transitions are valid (can't collect before complete)
5. Error handling is consistent — network errors, parse errors, missing fields
6. The adapter correctly delegates to SessionAdapterBase (for session adapters)

Respond with exactly one of:
- PASS — if the adapter correctly implements the contract
- FAIL {method and issue} — cite the specific contract violation
- WARN {concern} — if implementation is technically correct but fragile

## Trait Definition

{input.trait_def.content}

## Adapter Implementation

{input.adapter.content}
