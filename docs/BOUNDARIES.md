# Boundaries & Guardrails

Rules about what NOT to do. Each rule here exists because it was either violated or nearly violated in the past.

## Do Not Add Dependencies Without Justification

The dependency list is intentionally small. Before adding a crate:

- Check if `std` or an existing dependency already covers the need.
- `reqwest` is already included — don't add `ureq` or `hyper` for HTTP.
- `serde_json` handles all JSON — don't add `simd-json` or `json5`.

## Do Not Use HashMap for User-Visible Data

`HashMap` iteration order is non-deterministic. Any data that affects:

- JSON output
- SHA-256 hashing
- Test assertions comparing serialized output

must use `BTreeMap`. This is already the convention — don't regress.

## Do Not Eagerly Load File Content

The lazy loading pattern in `Artifact` and `Context` is intentional. Adding an `Artifact::from_file_with_content()` that reads immediately would break the design contract that validators only pay for what they use.

## Do Not Add Global Mutable State

No `static mut`, no `lazy_static` with interior mutability, no global registries. State flows through function arguments. The execution pipeline is a pure function of (config, artifact, context) → verdict.

## Do Not Merge Config Parsing and Validation

`parse_config()` and `validate_config()` are separate on purpose (see `docs/ARCHITECTURE.md`). Don't add validation logic to the serde deserialize path.

## Do Not Mock the Filesystem in Tests

Use `tempfile` to create real files. Mocking `std::fs` adds complexity and hides real behavior differences (permissions, encoding, symlinks).

## Do Not Swallow Errors

Every `unwrap()` in non-test code is a potential crash. Use `?` propagation or explicit error handling. The only acceptable `unwrap()` is on invariants that are structurally guaranteed (document with a comment explaining why).

## Security

- Never log or include artifact content in error messages — artifacts may contain secrets.
- Environment variable interpolation (`${VAR}`) in config must not execute shell commands.
- Script validator commands run via the system shell — always document this trust boundary.
