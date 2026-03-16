# module: types

Core data types for baton: artifacts, context, verdicts, and run options.

This module defines the value-layer types that every other module depends on. It has no dependencies on other baton modules (except `error`), making it the leaf of the dependency graph. The key abstractions are Artifact (lazy-loaded file/string with SHA-256 hashing), ContextItem and Context (named reference documents in a deterministic collection), Status/VerdictStatus (validator-level vs gate-level outcomes), ValidatorResult, Verdict (with JSON/human/summary output), and RunOptions.

## Public types and functions

| Type/Function                  | Purpose                                                 |
|--------------------------------|---------------------------------------------------------|
| `Artifact::from_file`          | Create artifact from filesystem path                    |
| `Artifact::from_string`        | Create artifact from inline string                      |
| `Artifact::from_bytes`         | Create artifact from raw bytes                          |
| `Artifact::get_content`        | Lazy content loading (reads file on first access)       |
| `Artifact::get_hash`           | SHA-256 hash, computed and cached on first call         |
| `Artifact::get_content_as_string` | Lossy UTF-8 conversion of content                    |
| `Artifact::absolute_path`      | Returns absolute path string if file-backed             |
| `Artifact::parent_dir`         | Returns parent directory string if file-backed          |
| `ContextItem::from_file`       | Create context item from filesystem path                |
| `ContextItem::from_string`     | Create context item from inline string                  |
| `ContextItem::get_content`     | Lazy content loading (reads file on first access)       |
| `ContextItem::get_hash`        | SHA-256 hash of content (not cached)                    |
| `ContextItem::absolute_path`   | Returns absolute path string if file-backed             |
| `Context::new`                 | Empty BTreeMap collection                               |
| `Context::add_file`            | Add file-backed context item                            |
| `Context::add_string`          | Add inline string context item                          |
| `Context::get_hash`            | Combined hash of all items in sorted key order          |
| `Status`                       | 5-variant enum: Pass, Fail, Warn, Skip, Error           |
| `VerdictStatus`                | 3-variant enum: Pass, Fail, Error                       |
| `ValidatorResult`              | Single validator outcome with name, status, feedback    |
| `Verdict`                      | Gate-level result with history and output formatters    |
| `RunOptions`                   | Runtime options for filtering and reporting              |

## Design notes

Artifact uses lazy content and hash loading. Content is not read from disk until `get_content` is called, and the hash is not computed until `get_hash` is called. Once computed, the hash is cached in the struct so subsequent calls return the same value even if the underlying file changes. This is intentional: the hash captures the artifact state at the time of first access, which is when baton records it in the verdict.

Context uses BTreeMap rather than HashMap so that iteration order is deterministic (sorted by key name). This matters because `get_hash` joins individual item hashes in iteration order — a HashMap would produce different combined hashes depending on internal bucket ordering, making verdicts non-reproducible.

VerdictStatus has only three variants (Pass, Fail, Error) while Status has five (adding Warn, Skip). This is a deliberate gate-level vs validator-level distinction. A single validator can warn or be skipped, but a gate as a whole either passes, fails, or errors. The mapping from validator statuses to a gate verdict is handled by `exec::compute_final_status`, not by this module.

Status::FromStr only accepts exact lowercase strings. This prevents ambiguity (e.g., "PASS" vs "Pass" vs "pass") and matches the serde representation. The error message includes the rejected value to aid debugging.

`to_human` intentionally hides feedback for Pass validators. When a gate has many validators, showing "all good" messages for every passing check creates visual noise. Only failures, warnings, and errors get their feedback displayed. Feedback is also truncated to 5 lines to keep terminal output manageable.

RunOptions::new() enables logging while Default (derived) disables it. The `new()` constructor is used by the CLI where logging is expected. The Default trait is used by tests and programmatic callers who typically don't want SQLite side effects.

---

## Artifact

Represents a file or in-memory content to be validated. Supports lazy content loading from disk and cached SHA-256 hashing.

### Sections

1. Construction (from_file, from_string, from_bytes)
2. Content access (get_content, get_content_as_string)
3. Hashing (get_hash)
4. Path accessors (absolute_path, parent_dir)

### Artifact: construction

Artifacts can be created from three sources: a filesystem path, an inline string, or raw bytes. File-backed artifacts validate the path at construction time but defer reading content until first access.

SPEC-TY-AF-001: from-file-rejects-nonexistent-path
  When the path does not exist on disk, `from_file` returns `Err(BatonError::ArtifactNotFound)` containing the path string. The existence check runs before the directory check.
  test: types::tests::artifact_from_file_not_found

SPEC-TY-AF-002: from-file-rejects-directory
  When the path exists but is a directory, `from_file` returns `Err(BatonError::ArtifactIsDirectory)` containing the path string. This check runs after the existence check, so a nonexistent path always gets ArtifactNotFound, never ArtifactIsDirectory.
  test: types::tests::artifact_from_file_is_directory

SPEC-TY-AF-003: from-file-converts-relative-to-absolute
  When a relative path is provided, `from_file` resolves it to an absolute path using `std::env::current_dir()`. The stored `path` field is always absolute after successful construction.
  test: types::tests::artifact_from_file_stores_absolute_path

SPEC-TY-AF-004: from-file-defers-content-read
  A successful `from_file` call sets `content` to None and `hash` to None. Content is not read from disk until `get_content` is called. This allows constructing an artifact for path validation without incurring I/O cost.
  test: IMPLICIT via types::tests::artifact_from_file_success (content only read after get_content call)

SPEC-TY-AF-005: from-file-stores-path
  After successful construction, `artifact.path` is `Some` containing the absolute path.
  test: types::tests::artifact_from_file_success

SPEC-TY-AF-006: from-string-stores-content-immediately
  `from_string` converts the string to bytes and stores it in `content` immediately. The `path` field is None. No filesystem access occurs.
  test: types::tests::artifact_from_string

SPEC-TY-AF-007: from-string-has-no-path
  An artifact created via `from_string` has `path` set to None.
  test: types::tests::artifact_absolute_path_from_string_is_none

SPEC-TY-AF-008: from-bytes-stores-content-immediately
  `from_bytes` takes ownership of a `Vec<u8>` and stores it directly in `content`. The `path` field is None.
  test: types::tests::artifact_from_bytes

SPEC-TY-AF-009: from-bytes-has-no-path
  An artifact created via `from_bytes` has `path` set to None.
  test: types::tests::artifact_from_bytes

SPEC-TY-AF-010: empty-file-is-valid-artifact
  A file with zero bytes is a valid artifact. `from_file` succeeds and `get_content` returns an empty slice.
  test: types::tests::artifact_empty_file_is_valid

### Artifact: content access

SPEC-TY-AF-020: get-content-reads-file-on-first-call
  For file-backed artifacts, `get_content` reads the file from disk on its first invocation and caches the bytes. Subsequent calls return the cached content without re-reading.
  test: types::tests::artifact_from_file_success

SPEC-TY-AF-021: get-content-returns-cached-for-string-artifacts
  For string/bytes artifacts, `get_content` returns the content that was provided at construction time. No filesystem access occurs.
  test: types::tests::artifact_from_string

SPEC-TY-AF-022: get-content-panics-if-no-path-and-no-content
  If both `path` and `content` are None, `get_content` panics with "Artifact must have path or content". This state is unreachable through the public API — all constructors set at least one of path or content.
  test: UNTESTED (unreachable through public API)

SPEC-TY-AF-023: get-content-as-string-uses-lossy-utf8
  `get_content_as_string` converts the raw bytes to a String using `String::from_utf8_lossy`. Invalid UTF-8 sequences are replaced with U+FFFD (replacement character). This never fails — it always returns Ok.
  test: types::tests::artifact_get_content_as_string_lossy

### Artifact: hashing

SPEC-TY-AF-030: get-hash-returns-sha256-hex
  `get_hash` returns the SHA-256 digest of the content as a lowercase hex-encoded string. The hash is always exactly 64 characters of ASCII hex digits.
  test: types::tests::artifact_hash_is_64_hex_chars

SPEC-TY-AF-031: get-hash-is-deterministic
  Two artifacts with identical content produce identical hashes, regardless of construction method.
  test: types::tests::artifact_hash_deterministic

SPEC-TY-AF-032: get-hash-differs-for-different-content
  Two artifacts with different content produce different hashes.
  test: types::tests::artifact_hash_differs_for_different_content

SPEC-TY-AF-033: get-hash-is-cached
  Once computed, the hash is cached in the struct. Subsequent calls to `get_hash` return the cached value. If the underlying file is modified after the first `get_hash` call, the returned hash does not change.
  test: types::tests::artifact_hash_cached_on_second_call

SPEC-TY-AF-034: get-hash-triggers-lazy-content-load
  Calling `get_hash` on a file-backed artifact that has not yet had `get_content` called will trigger the lazy content read as a side effect, because the hash is computed from the content bytes.
  test: IMPLICIT via types::tests::artifact_hash_deterministic (hash computed without prior get_content)

### Artifact: path accessors

SPEC-TY-AF-040: absolute-path-returns-string-for-file-backed
  For file-backed artifacts, `absolute_path` returns `Some(String)` containing the absolute path. The path is always absolute because `from_file` normalizes it.
  test: types::tests::artifact_absolute_path_from_file

SPEC-TY-AF-041: absolute-path-returns-none-for-non-file
  For string or bytes artifacts, `absolute_path` returns None.
  test: types::tests::artifact_absolute_path_from_string_is_none

SPEC-TY-AF-042: parent-dir-returns-parent-for-file-backed
  For file-backed artifacts, `parent_dir` returns `Some(String)` containing the parent directory of the file path. The returned path is a valid directory.
  test: types::tests::artifact_parent_dir_from_file

SPEC-TY-AF-043: parent-dir-returns-none-for-non-file
  For string or bytes artifacts, `parent_dir` returns None.
  test: types::tests::artifact_absolute_path_from_string_is_none

---

## ContextItem

A named reference document provided as context for validation. Mirrors Artifact's lazy-loading pattern but stores content as String (UTF-8) rather than raw bytes, and includes a name field.

SPEC-TY-CI-001: from-file-rejects-nonexistent-path
  When the path does not exist on disk, `from_file` returns `Err(BatonError::ContextNotFound)` containing both the item name and the path string.
  test: types::tests::context_item_from_file_not_found

SPEC-TY-CI-002: from-file-rejects-directory
  When the path exists but is a directory, `from_file` returns `Err(BatonError::ContextIsDirectory)` containing both the item name and the path string.
  test: types::tests::context_item_from_file_is_directory

SPEC-TY-CI-003: from-file-converts-relative-to-absolute
  When a relative path is provided, `from_file` resolves it to an absolute path using `std::env::current_dir()`. The stored `path` field is always absolute.
  test: types::tests::context_item_from_file_stores_absolute_path

SPEC-TY-CI-004: from-file-defers-content-read
  A successful `from_file` call sets `content` to None. Content is not read from disk until `get_content` is called.
  test: IMPLICIT via types::tests::context_add_file_success

SPEC-TY-CI-005: from-string-stores-content-immediately
  `from_string` stores the provided String directly. The `path` field is None.
  test: types::tests::context_item_from_string

SPEC-TY-CI-006: from-string-has-no-path
  A context item created via `from_string` has `absolute_path()` returning None.
  test: types::tests::context_item_from_string_has_no_path

SPEC-TY-CI-007: get-content-reads-file-on-first-call
  For file-backed items, `get_content` reads the file as a UTF-8 string on first invocation and caches the result.
  test: types::tests::context_add_file_success

SPEC-TY-CI-008: get-content-panics-if-no-path-and-no-content
  If both `path` and `content` are None, `get_content` panics with "ContextItem must have path or content". This state is unreachable through the public API.
  test: UNTESTED (unreachable through public API)

SPEC-TY-CI-009: get-hash-returns-sha256-hex
  `get_hash` returns the SHA-256 digest of the content string as a lowercase hex-encoded string.
  test: IMPLICIT via types::tests::context_hash_deterministic

SPEC-TY-CI-010: get-hash-is-not-cached
  Unlike Artifact, ContextItem does not cache its hash. Each call to `get_hash` recomputes from content. This is a simplification — context items are typically hashed once during pre-flight.
  test: types::tests::context_item_get_hash_recomputes_same_value

SPEC-TY-CI-011: absolute-path-returns-string-for-file-backed
  For file-backed items, `absolute_path` returns `Some(String)` containing the absolute path.
  test: types::tests::context_item_absolute_path

SPEC-TY-CI-012: absolute-path-returns-none-for-string
  For inline string items, `absolute_path` returns None.
  test: types::tests::context_item_from_string_has_no_path

---

## Context

Ordered collection of named context items. Uses `BTreeMap<String, ContextItem>` for deterministic iteration order.

The choice of BTreeMap over HashMap is load-bearing for hash reproducibility. If two baton invocations provide the same context items (possibly in different order), they must produce the same `context_hash` in the verdict. BTreeMap guarantees sorted-key iteration, making the combined hash independent of insertion order.

SPEC-TY-CX-001: new-creates-empty-collection
  `Context::new()` returns a context with an empty items map.
  test: IMPLICIT via types::tests::context_hash_empty

SPEC-TY-CX-002: add-file-delegates-to-context-item
  `add_file` creates a `ContextItem::from_file` and inserts it into the map by name. It propagates any error from `ContextItem::from_file` (e.g., file not found, is directory).
  test: types::tests::context_add_file_success

SPEC-TY-CX-003: add-string-inserts-inline-item
  `add_string` creates a `ContextItem::from_string` and inserts it into the map by name. This operation is infallible (no Result return).
  test: IMPLICIT via types::tests::context_hash_deterministic

SPEC-TY-CX-004: add-overwrites-existing-key
  If an item with the same name already exists, `add_file` or `add_string` replaces it silently. BTreeMap::insert overwrites on duplicate keys.
  test: types::tests::context_add_duplicate_replaces_silently

SPEC-TY-CX-005: get-hash-joins-item-hashes-with-colon
  `get_hash` computes the SHA-256 hash of each item (in sorted key order), joins them with ":" as separator, then computes the SHA-256 of the joined string.
  test: IMPLICIT via types::tests::context_hash_deterministic

SPEC-TY-CX-006: get-hash-is-deterministic-regardless-of-insertion-order
  Two Context collections with the same items inserted in different order produce the same hash, because BTreeMap sorts by key.
  test: types::tests::context_hash_deterministic

SPEC-TY-CX-007: get-hash-of-empty-context
  An empty context produces the SHA-256 of the empty string (since joining zero hashes with ":" yields "").
  test: types::tests::context_hash_empty

SPEC-TY-CX-008: get-hash-differs-with-different-content
  Changing the content of an item (same key name) produces a different combined hash.
  test: types::tests::context_hash_differs_with_different_content

SPEC-TY-CX-009: get-hash-differs-when-values-swapped-between-keys
  Two contexts with the same values assigned to different keys produce different hashes. The per-item hashes are the same, but their positions in the colon-joined string differ because the keys sort differently.
  test: types::tests::context_hash_differs_with_different_key_names

SPEC-TY-CX-010: same-content-different-key-name-produces-same-hash-for-single-item
  When there is only one item, the combined hash depends only on the item's content hash, not its key name. Two single-item contexts with different key names but identical content produce the same combined hash. Key names affect ordering, not individual item hashes.
  test: types::tests::context_single_item_hash_ignores_key_name

---

## Status

Five-variant enum representing the outcome of a single validator: Pass, Fail, Warn, Skip, Error.

SPEC-TY-ST-001: display-is-lowercase
  `Status::Display` renders each variant as its lowercase name: "pass", "fail", "warn", "skip", "error".
  test: types::tests::status_display

SPEC-TY-ST-002: from-str-accepts-exact-lowercase
  `Status::FromStr` accepts exactly the five lowercase strings: "pass", "fail", "warn", "skip", "error". Each maps to the corresponding variant.
  test: types::tests::status_from_str

SPEC-TY-ST-003: from-str-rejects-non-lowercase
  Any casing other than all-lowercase is rejected. "Pass", "FAIL", "Error" all return Err. This is intentional — it prevents ambiguity and matches the serde representation.
  test: types::tests::status_from_str_rejects_uppercase

SPEC-TY-ST-004: from-str-error-message-includes-value
  The Err string from `FromStr` contains the invalid input value, formatted as `Invalid status: '<value>'`.
  test: types::tests::status_from_str_error_includes_value

SPEC-TY-ST-005: serde-uses-lowercase
  Serialization and deserialization use `#[serde(rename_all = "lowercase")]`, matching the Display/FromStr representations.
  test: IMPLICIT via types::tests::verdict_to_json_roundtrip (Status serialized as part of ValidatorResult)

SPEC-TY-ST-006: status-is-copy
  Status derives Copy, allowing it to be passed by value without cloning.
  test: UNTESTED (structural property, not behavioral)

---

## VerdictStatus

Three-variant enum representing the gate-level outcome: Pass, Fail, Error. No Warn or Skip variants exist at the gate level.

The absence of Warn and Skip is deliberate. A gate either passes, fails, or encounters an error. Warnings from individual validators do not propagate to the gate verdict (they are treated as pass by `compute_final_status`). Skipped validators are excluded from consideration entirely.

SPEC-TY-VS-001: exit-code-mapping
  `exit_code()` maps Pass to 0, Fail to 1, Error to 2. These are used as process exit codes by the CLI.
  test: types::tests::verdict_status_exit_codes

SPEC-TY-VS-002: display-is-lowercase
  `VerdictStatus::Display` renders each variant as its lowercase name: "pass", "fail", "error".
  test: types::tests::verdict_status_display

SPEC-TY-VS-003: serde-uses-lowercase
  Serialization and deserialization use `#[serde(rename_all = "lowercase")]`.
  test: IMPLICIT via types::tests::verdict_json_roundtrip_full

---

## ValidatorResult

Outcome of a single validator run. A plain data struct with no methods.

| Field        | Type             | Description                                     |
|--------------|------------------|-------------------------------------------------|
| `name`       | `String`         | Validator name from config                      |
| `status`     | `Status`         | Outcome status                                  |
| `feedback`   | `Option<String>` | Human-readable feedback text, if any            |
| `duration_ms`| `i64`            | Wall-clock execution time in milliseconds       |
| `cost`       | `Option<Cost>`   | LLM token/cost metadata, None for non-LLM       |

SPEC-TY-VR-001: serializable-and-deserializable
  ValidatorResult derives Serialize and Deserialize for JSON roundtripping as part of Verdict.
  test: IMPLICIT via types::tests::verdict_json_roundtrip_full

SPEC-TY-VR-002: cost-field-optional
  The `cost` field is None for script and human validators. When present (LLM validators), it contains token counts, model name, and estimated USD cost.
  test: types::tests::verdict_json_roundtrip_full

---

## Cost

Token usage and cost metadata from an LLM validator call. A plain data struct with no methods. All fields are optional.

| Field           | Type             | Description                          |
|-----------------|------------------|--------------------------------------|
| `input_tokens`  | `Option<i64>`    | Number of input tokens consumed      |
| `output_tokens` | `Option<i64>`    | Number of output tokens generated    |
| `model`         | `Option<String>` | Model identifier                     |
| `estimated_usd` | `Option<f64>`    | Estimated cost in USD                |

SPEC-TY-CO-001: default-is-all-none
  Cost derives Default, which sets all fields to None.
  test: UNTESTED (structural property)

SPEC-TY-CO-002: serializable-and-deserializable
  Cost derives Serialize and Deserialize. When serialized as part of a ValidatorResult, None fields are included as JSON null.
  test: types::tests::verdict_json_roundtrip_full

---

## Verdict

Final gate-level result containing all validator outcomes and aggregate metadata.

| Field            | Type                   | Description                                    |
|------------------|------------------------|------------------------------------------------|
| `status`         | `VerdictStatus`        | Gate-level pass/fail/error                     |
| `gate`           | `String`               | Gate name from config                          |
| `failed_at`      | `Option<String>`       | Name of first failing validator, if any        |
| `feedback`       | `Option<String>`       | Feedback from the failing validator            |
| `duration_ms`    | `i64`                  | Total gate execution time                      |
| `timestamp`      | `DateTime<Utc>`        | When the verdict was produced                  |
| `artifact_hash`  | `String`               | SHA-256 of the artifact content                |
| `context_hash`   | `String`               | Combined SHA-256 of all context items          |
| `warnings`       | `Vec<String>`          | Non-fatal warnings collected during execution  |
| `suppressed`     | `Vec<String>`          | Names of status-suppressed validators          |
| `history`        | `Vec<ValidatorResult>` | All validator results in pipeline order        |

### Verdict: to_json

SPEC-TY-VD-001: to-json-produces-pretty-printed-json
  `to_json` serializes the entire Verdict struct as pretty-printed JSON using `serde_json::to_string_pretty`. The output is valid JSON that can be deserialized back into a Verdict.
  test: types::tests::verdict_to_json_roundtrip

SPEC-TY-VD-002: to-json-preserves-all-fields
  All Verdict fields survive a JSON roundtrip, including nested structures like Cost within ValidatorResult, optional fields (Some and None), and vector fields (warnings, suppressed, history).
  test: types::tests::verdict_json_roundtrip_full

SPEC-TY-VD-003: to-json-panics-on-serialization-failure
  `to_json` calls `expect` on the serialization result. In practice this never fails because all fields are serializable, but the contract is panic rather than Result.
  test: UNTESTED (unreachable in practice)

### Verdict: to_human

SPEC-TY-VD-010: to-human-shows-status-icons
  Each validator result is prefixed with a status icon: Pass=checkmark (U+2713), Fail=ballot-x (U+2717), Warn=exclamation, Skip=em-dash (U+2014), Error=letter-E.
  test: types::tests::verdict_to_human_all_status_icons

SPEC-TY-VD-011: to-human-shows-skip-label
  Skipped validators display "(skipped)" after the validator name, in addition to the em-dash icon.
  test: types::tests::verdict_to_human_skip_label

SPEC-TY-VD-012: to-human-shows-duration
  Each validator line includes the duration in the format "(Nms)" after the name (and skip label, if present).
  test: types::tests::verdict_to_human

SPEC-TY-VD-013: to-human-hides-pass-feedback
  When a validator has Status::Pass, its feedback is not displayed even if present. This is intentional to reduce visual noise — passing validators don't need explanation.
  test: types::tests::verdict_to_human_pass_feedback_not_shown

SPEC-TY-VD-014: to-human-shows-feedback-for-non-pass
  When a validator has Status::Fail, Warn, Skip, or Error and has feedback, the feedback is displayed indented under the validator line.
  test: types::tests::verdict_to_human

SPEC-TY-VD-015: to-human-truncates-feedback-to-5-lines
  Feedback is truncated to the first 5 lines. Lines 6 and beyond are silently dropped. No truncation indicator is added.
  test: types::tests::verdict_to_human_feedback_truncated_to_5_lines

SPEC-TY-VD-016: to-human-feedback-indented
  Each line of feedback is indented with 4 spaces.
  test: IMPLICIT via types::tests::verdict_to_human

SPEC-TY-VD-017: to-human-ends-with-verdict-line
  The final line is formatted as "  VERDICT: {STATUS}" with the status in uppercase. If `failed_at` is present, it appends " (failed at: {name})".
  test: types::tests::verdict_to_human

SPEC-TY-VD-018: to-human-lines-joined-with-newline
  All lines are joined with "\n". There is no trailing newline.
  test: types::tests::verdict_to_human_no_trailing_newline

### Verdict: to_summary

SPEC-TY-VD-020: to-summary-pass-returns-bare-pass
  When status is Pass, `to_summary` returns the literal string "PASS" with no additional information.
  test: types::tests::verdict_to_summary_pass

SPEC-TY-VD-021: to-summary-fail-includes-failed-at-and-feedback
  When status is Fail, `to_summary` returns "FAIL at {failed_at}: {feedback}". The status is uppercased.
  test: types::tests::verdict_to_summary_fail

SPEC-TY-VD-022: to-summary-error-includes-failed-at-and-feedback
  When status is Error, `to_summary` returns "ERROR at {failed_at}: {feedback}". Error and Fail follow the same format with different prefixes.
  test: types::tests::verdict_to_summary_error

SPEC-TY-VD-023: to-summary-uses-first-line-of-multiline-feedback
  When feedback contains multiple lines, only the first line is included in the summary.
  test: types::tests::verdict_to_summary_multiline_feedback_uses_first_line

SPEC-TY-VD-024: to-summary-omits-colon-when-no-feedback
  When feedback is None or empty, the ": {feedback}" suffix is omitted entirely. The output is "FAIL at {name}" with no trailing colon or space.
  test: types::tests::verdict_to_summary_fail_no_feedback

SPEC-TY-VD-025: to-summary-uses-unknown-when-failed-at-is-none
  When `failed_at` is None (which should not happen for Fail/Error verdicts in practice), the string "unknown" is used as the validator name.
  test: types::tests::verdict_to_summary_fail_no_failed_at_uses_unknown

---

## RunOptions

Runtime options controlling which validators to run and how results are reported.

| Field                | Type                 | Default (new) | Default (Default) |
|----------------------|----------------------|---------------|--------------------|
| `run_all`            | `bool`               | false         | false              |
| `only`               | `Option<Vec<String>>`| None          | None               |
| `skip`               | `Option<Vec<String>>`| None          | None               |
| `tags`               | `Option<Vec<String>>`| None          | None               |
| `timeout`            | `Option<u64>`        | None          | None               |
| `log`                | `bool`               | true          | false              |
| `suppressed_statuses`| `Vec<Status>`        | empty         | empty              |

SPEC-TY-RO-001: new-enables-logging
  `RunOptions::new()` sets `log` to true. All other fields are at their Default values (false, None, empty vec).
  test: types::tests::run_options_new_enables_logging

SPEC-TY-RO-002: default-disables-logging
  `RunOptions::default()` (via derived Default) sets `log` to false. This is the only difference from `new()`.
  test: types::tests::run_options_default_disables_logging

SPEC-TY-RO-003: new-sets-all-other-fields-to-default
  `RunOptions::new()` sets `run_all` to false, `only`/`skip`/`tags`/`timeout` to None, and `suppressed_statuses` to empty vec.
  test: types::tests::run_options_new_enables_logging
