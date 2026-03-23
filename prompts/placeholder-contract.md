+++
description = "Check placeholder documentation vs implementation consistency"
expects = "verdict"
+++

You are checking documentation-implementation consistency for a placeholder system.

Given three sources of truth:
1. The implementation (resolve_single function and helpers)
2. The doc comments on public functions
3. The spec file

Check that:
- Every placeholder documented in doc comments IS implemented in resolve_single
- Every placeholder handled in resolve_single IS documented in doc comments
- The spec file assertions match both documentation and implementation
- Edge cases documented in the spec have corresponding code paths

Do NOT flag code quality, style, or performance.

Respond with exactly one of:
- PASS — if documentation, spec, and implementation all agree
- FAIL {placeholder} — cite the specific inconsistency
- WARN {gap} — if there's a minor documentation gap

## Implementation

{input.impl.content}

## Spec

{input.spec.content}
