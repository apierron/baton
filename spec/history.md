# module: history

SQLite-based verdict history storage. Persists verdicts and individual validator results with indexes for efficient querying by gate, status, artifact hash, and timestamp.

This module is intentionally thin — it translates between the in-memory `Verdict`/`ValidatorResult` types and SQLite rows. There is no business logic here beyond schema creation and parameterized queries.

## Public functions

| Function            | Purpose                                            |
|---------------------|----------------------------------------------------|
| `init_db`           | Initialize SQLite DB with schema (CREATE IF NOT EXISTS) |
| `store_verdict`     | Insert verdict + validator_results rows, returns UUID   |
| `query_recent`      | Query verdicts with optional gate/status filters, limit |
| `query_by_artifact` | Query verdicts by artifact_hash                         |

## Types

| Type              | Purpose                                         |
|-------------------|-------------------------------------------------|
| `VerdictSummary`  | Projection row for history queries (id, timestamp, gate, status, failed_at, feedback, duration_ms, artifact_hash) |

## Design notes

WAL journal mode is set immediately after opening the connection. This is required for concurrent read/write access — without it, readers would block writers. The trade-off is a WAL file on disk alongside the database, but baton databases are small and short-lived enough that this is not a concern.

`VerdictSummary` is a separate struct from `Verdict` because query results project a subset of columns. The full verdict JSON is stored in the `verdict_json` column for archival but is not loaded by the query functions — callers who need the full verdict can query it separately.

store_verdict uses UUIDs (v4) for both verdict and validator_result primary keys rather than auto-increment integers. This avoids conflicts when merging history databases and makes IDs meaningful outside the database context (e.g., in logs or API responses).

---

## init_db

Opens or creates a SQLite database at the given path, sets WAL mode, and creates the schema (tables and indexes). Returns the open connection.

### Schema

The schema consists of two tables and six indexes:

- **verdicts**: id (TEXT PK), timestamp, gate, status, failed_at, feedback, duration_ms, artifact_hash, context_hash, warnings_json, suppressed_json, verdict_json
- **validator_results**: id (TEXT PK), verdict_id (TEXT FK -> verdicts.id), name, status, feedback, duration_ms, input_tokens, output_tokens, model, estimated_usd
- **Indexes**: idx_verdicts_gate, idx_verdicts_status, idx_verdicts_artifact, idx_verdicts_context, idx_verdicts_timestamp, idx_vresults_verdict

All CREATE statements use IF NOT EXISTS, making init_db idempotent.

SPEC-HI-ID-001: open-connection-or-error
  When the SQLite connection cannot be opened (e.g., invalid path, permission denied), init_db returns Err(BatonError::DatabaseError) with a message containing "Failed to open database".
  test: UNTESTED (filesystem permission errors are platform-dependent and not exercised)

SPEC-HI-ID-002: wal-mode-set
  After opening the connection, init_db sets the journal_mode pragma to WAL. If the pragma update fails, it returns Err(BatonError::DatabaseError) with "Failed to set WAL mode".
  test: history::tests::wal_mode_enabled

SPEC-HI-ID-003: verdicts-table-created
  init_db creates the verdicts table with 12 columns: id (TEXT PK), timestamp (TEXT NOT NULL), gate (TEXT NOT NULL), status (TEXT NOT NULL), failed_at (TEXT nullable), feedback (TEXT nullable), duration_ms (INTEGER NOT NULL), artifact_hash (TEXT NOT NULL), context_hash (TEXT NOT NULL), warnings_json (TEXT nullable), suppressed_json (TEXT nullable), verdict_json (TEXT NOT NULL).
  test: history::tests::init_db_creates_schema

SPEC-HI-ID-004: validator-results-table-created
  init_db creates the validator_results table with 10 columns: id (TEXT PK), verdict_id (TEXT NOT NULL, FK to verdicts.id), name (TEXT NOT NULL), status (TEXT NOT NULL), feedback (TEXT nullable), duration_ms (INTEGER NOT NULL), input_tokens (INTEGER nullable), output_tokens (INTEGER nullable), model (TEXT nullable), estimated_usd (REAL nullable).
  test: history::tests::init_db_creates_schema

SPEC-HI-ID-005: six-indexes-created
  init_db creates six indexes: idx_verdicts_gate (gate), idx_verdicts_status (status), idx_verdicts_artifact (artifact_hash), idx_verdicts_context (context_hash), idx_verdicts_timestamp (timestamp) on the verdicts table, and idx_vresults_verdict (verdict_id) on the validator_results table. All use IF NOT EXISTS.
  test: history::tests::index_existence_after_init_db

SPEC-HI-ID-006: schema-creation-failure-returns-error
  If the CREATE TABLE / CREATE INDEX batch fails, init_db returns Err(BatonError::DatabaseError) with "Failed to create schema".
  test: UNTESTED (would require a mock or deliberately broken connection)

SPEC-HI-ID-007: idempotent-init
  Calling init_db twice on the same database file succeeds. The second call does not destroy data written after the first call. This is guaranteed by CREATE TABLE IF NOT EXISTS and CREATE INDEX IF NOT EXISTS.
  test: history::tests::idempotent_init

SPEC-HI-ID-008: zero-byte-file-treated-as-new-db
  When init_db is called on a zero-byte file, SQLite treats it as a new database. Schema creation succeeds and the database is fully functional.
  test: history::tests::zero_byte_db_file

SPEC-HI-ID-009: corrupted-file-returns-error
  When init_db is called on a file containing non-SQLite data (e.g., arbitrary text), the schema creation step fails and init_db returns Err(BatonError::DatabaseError).
  test: history::tests::corrupted_db_file_returns_error

SPEC-HI-ID-010: truncated-file-returns-error
  When init_db is called on a file that was truncated mid-header (e.g., first 10 bytes of a valid SQLite header), the operation fails and init_db returns Err(BatonError::DatabaseError).
  test: history::tests::truncated_db_file_returns_error

SPEC-HI-ID-011: dropped-tables-recreated
  If tables are manually dropped from an existing database file, a subsequent init_db call recreates them via CREATE TABLE IF NOT EXISTS. The database is fully functional after re-initialization.
  test: history::tests::missing_tables_after_manual_drop

SPEC-HI-ID-012: returns-open-connection
  On success, init_db returns Ok(Connection) — the caller receives an open, ready-to-use SQLite connection with the schema already applied.
  test: IMPLICIT via all tests (every test uses the returned connection)

---

## store_verdict

Inserts a verdict and its associated validator results into the database. Returns the generated verdict UUID.

SPEC-HI-SV-001: verdict-id-is-uuid-v4
  store_verdict generates a UUID v4 string as the verdict's primary key. Each call produces a unique ID.
  test: history::tests::store_verdict_returns_unique_ids

SPEC-HI-SV-002: verdict-row-inserted
  store_verdict inserts one row into the verdicts table with all 12 columns populated from the Verdict struct. The timestamp is stored as an RFC 3339 string (via `verdict.timestamp.to_rfc3339()`). The status is stored as the Display string of VerdictStatus (e.g., "pass", "fail", "error").
  test: history::tests::store_and_query_verdict

SPEC-HI-SV-003: warnings-serialized-as-json
  The verdict's warnings Vec is serialized to a JSON array string and stored in the warnings_json column. An empty Vec serializes to "[]".
  test: history::tests::warnings_json_column_stores_warnings

SPEC-HI-SV-004: suppressed-serialized-as-json
  The verdict's suppressed Vec is serialized to a JSON array string and stored in the suppressed_json column. An empty Vec serializes to "[]".
  test: history::tests::suppressed_json_column_stores_suppressed

SPEC-HI-SV-005: verdict-json-stored
  The full verdict is serialized via `verdict.to_json()` and stored in the verdict_json column. This column contains enough information to reconstruct the complete Verdict struct.
  test: history::tests::verdict_json_column_is_parseable

SPEC-HI-SV-006: one-row-per-validator-result
  For each entry in `verdict.history`, store_verdict inserts one row into the validator_results table. The row is linked to the verdict via verdict_id.
  test: history::tests::validator_results_linked_to_verdict
  test: history::tests::multiple_validator_results_stored

SPEC-HI-SV-007: validator-result-id-is-uuid-v4
  Each validator_result row gets its own UUID v4 primary key, independent of the verdict's UUID.
  test: UNTESTED (individual result UUIDs are not inspected)

SPEC-HI-SV-008: cost-fields-from-some
  When a ValidatorResult has `cost: Some(Cost { ... })`, the cost fields (input_tokens, output_tokens, model, estimated_usd) are extracted and stored in the corresponding columns.
  test: history::tests::store_verdict_with_cost
  test: history::tests::multiple_validator_results_stored

SPEC-HI-SV-009: cost-fields-null-when-none
  When a ValidatorResult has `cost: None`, all four cost columns (input_tokens, output_tokens, model, estimated_usd) are stored as NULL.
  test: history::tests::null_cost_columns_when_cost_is_none

The cost extraction uses a match on `&result.cost` that maps `None` to a tuple of four `None` values. This ensures each Option field inside Cost is mapped individually — a Cost struct where `input_tokens` is Some but `model` is None will store the tokens but leave model as NULL.

SPEC-HI-SV-010: insert-verdict-failure-returns-error
  If the INSERT INTO verdicts statement fails, store_verdict returns Err(BatonError::DatabaseError) with "Failed to insert verdict".
  test: UNTESTED

SPEC-HI-SV-011: insert-result-failure-returns-error
  If any INSERT INTO validator_results statement fails, store_verdict returns Err(BatonError::DatabaseError) with "Failed to insert validator result". Earlier inserts (including the verdict row and previous result rows) are not rolled back — there is no explicit transaction wrapping the inserts.
  test: UNTESTED

Note: the lack of an explicit transaction in store_verdict means a failure mid-way through inserting validator_results will leave the verdict row and some result rows committed. This is a known simplification — the verdict_json column contains the full truth, so partial result rows are recoverable but may cause validator_results counts to be less than expected.

SPEC-HI-SV-012: timestamp-stored-as-rfc3339
  The verdict's `chrono::DateTime<Utc>` timestamp is converted to an RFC 3339 string via `.to_rfc3339()` before storage. This ensures timestamps sort lexicographically in the same order as chronologically.
  test: IMPLICIT via history::tests::query_recent_returns_newest_first (ordering depends on correct timestamp format)

SPEC-HI-SV-013: returns-verdict-id-on-success
  On success, store_verdict returns Ok(String) containing the UUID that was used as the verdict's primary key.
  test: history::tests::store_and_query_verdict

---

## query_recent

Queries verdicts with optional gate and/or status filters, limited to N results, ordered by timestamp descending (newest first).

### Dynamic SQL construction

query_recent builds its SQL string dynamically based on which filters are provided. The WHERE clause is only appended when at least one filter is present. Parameters are bound positionally.

SPEC-HI-QR-001: no-filters-returns-all
  When both gate and status are None, query_recent returns all verdicts (up to limit), ordered by timestamp DESC.
  test: history::tests::store_and_query_verdict

SPEC-HI-QR-002: gate-filter
  When gate is Some, only verdicts whose gate column matches the provided string are returned. The comparison is exact (case-sensitive, no wildcards).
  test: history::tests::query_by_gate

SPEC-HI-QR-003: status-filter
  When status is Some, only verdicts whose status column matches the provided string are returned. The comparison is exact.
  test: history::tests::query_by_status

SPEC-HI-QR-004: gate-and-status-combined
  When both gate and status are Some, the two conditions are combined with AND. Only verdicts matching both filters are returned.
  test: history::tests::query_by_gate_and_status_no_match (tests the no-match case)

SPEC-HI-QR-005: ordered-by-timestamp-desc
  Results are always ordered by timestamp DESC (newest first), regardless of filters.
  test: history::tests::query_recent_returns_newest_first

SPEC-HI-QR-006: limit-applied
  The LIMIT clause restricts the number of returned rows. Requesting 3 from a database with 5 rows returns exactly 3.
  test: history::tests::query_limit

SPEC-HI-QR-007: limit-zero-returns-empty
  When limit is 0, the SQL LIMIT 0 clause produces zero rows, even if matching verdicts exist.
  test: history::tests::query_recent_empty_db_with_limit_zero

SPEC-HI-QR-008: empty-db-returns-empty-vec
  When the database has no verdicts, query_recent returns Ok(Vec::new()), not an error.
  test: history::tests::query_recent_empty_db

SPEC-HI-QR-009: no-match-returns-empty-vec
  When filters are provided but no verdicts match, query_recent returns Ok(Vec::new()), not an error.
  test: history::tests::query_by_gate_no_match
  test: history::tests::query_by_status_no_match
  test: history::tests::query_by_gate_and_status_no_match

SPEC-HI-QR-010: returns-verdict-summary
  Each returned row is mapped to a VerdictSummary struct with fields: id, timestamp, gate, status, failed_at (Option), feedback (Option), duration_ms, artifact_hash. The verdict_json, context_hash, warnings_json, and suppressed_json columns are not included in the projection.
  test: IMPLICIT via history::tests::store_and_query_verdict (asserts gate and status fields)

SPEC-HI-QR-011: prepare-failure-returns-error
  If the SQL statement cannot be prepared, query_recent returns Err(BatonError::DatabaseError) with "Query error".
  test: UNTESTED

SPEC-HI-QR-012: row-read-failure-returns-error
  If an individual row cannot be read (e.g., type mismatch), query_recent returns Err(BatonError::DatabaseError) with "Row error".
  test: UNTESTED

---

## query_by_artifact

Queries all verdicts matching a specific artifact hash, ordered by timestamp descending.

Unlike query_recent, this function uses a fixed SQL query (no dynamic WHERE construction) and has no limit parameter — it returns all matching rows.

SPEC-HI-QA-001: filters-by-artifact-hash
  Only verdicts whose artifact_hash column exactly matches the provided string are returned.
  test: history::tests::query_by_artifact_hash

SPEC-HI-QA-002: ordered-by-timestamp-desc
  Results are ordered by timestamp DESC (newest first).
  test: history::tests::query_by_artifact_ordering_newest_first

SPEC-HI-QA-003: no-match-returns-empty-vec
  When no verdicts have the given artifact_hash, query_by_artifact returns Ok(Vec::new()), not an error.
  test: history::tests::query_by_artifact_hash (tests "nonexistent" hash)
  test: history::tests::query_by_artifact_no_match
  test: history::tests::query_by_artifact_empty_db

SPEC-HI-QA-004: returns-verdict-summary
  Each returned row is mapped to a VerdictSummary with the same 8-field projection as query_recent.
  test: IMPLICIT via history::tests::query_by_artifact_hash

SPEC-HI-QA-005: no-limit
  query_by_artifact has no limit parameter. All matching rows are returned. This is intentional — artifact-level queries are expected to return a small number of results (one per gate run against that artifact version).
  test: history::tests::query_by_artifact_returns_all_rows

SPEC-HI-QA-006: prepare-failure-returns-error
  If the SQL statement cannot be prepared, returns Err(BatonError::DatabaseError) with "Query error".
  test: UNTESTED

SPEC-HI-QA-007: row-read-failure-returns-error
  If an individual row cannot be read, returns Err(BatonError::DatabaseError) with "Row error".
  test: UNTESTED

---

## VerdictSummary

A plain data struct returned by the query functions. It projects a subset of the verdicts table columns.

SPEC-HI-VS-001: fields
  VerdictSummary has 8 fields: id (String), timestamp (String), gate (String), status (String), failed_at (Option<String>), feedback (Option<String>), duration_ms (i64), artifact_hash (String).
  test: IMPLICIT via all query tests

SPEC-HI-VS-002: derives-debug-clone
  VerdictSummary derives Debug and Clone.
  test: history::tests::verdict_summary_debug_and_clone

---

## Concurrency

WAL mode enables concurrent readers and a single writer. The history module does not set busy_timeout — callers are responsible for configuring retry behavior when contention occurs.

SPEC-HI-CC-001: concurrent-writes-succeed-with-busy-timeout
  Multiple threads writing to the same database file via separate connections succeed when each connection has a busy_timeout configured. All writes are durable and queryable afterward.
  test: history::tests::concurrent_writes_via_separate_connections

SPEC-HI-CC-002: concurrent-read-write-succeeds
  A reader and writer operating concurrently via separate connections both succeed when busy_timeout is configured. The reader sees a consistent snapshot (at least the initial seed data). WAL mode ensures readers do not block writers.
  test: history::tests::concurrent_read_write
