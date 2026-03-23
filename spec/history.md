# module: history

SQLite-based invocation history storage. Every `baton check` invocation is recorded with enough detail to reconstruct exactly what happened: which validators ran, against which files, with what content hashes, and what verdicts they produced.

The three-table schema mirrors the three-layer model: invocations (top-level), gate_results (orchestration outcomes), validator_runs (individual stateless function executions with their input files). The input file hashes recorded per validator run are what would power future incremental checking.

## Public functions

| Function               | Purpose                                                 |
|------------------------|---------------------------------------------------------|
| `init_db`              | Initialize SQLite DB with schema (CREATE IF NOT EXISTS) |
| `store_invocation`     | Insert invocation + gate_results + validator_runs rows  |
| `query_recent`         | Query invocations with optional gate/status filters     |
| `query_by_file`        | Query validator runs by file path                       |
| `query_by_hash`        | Query validator runs by content hash                    |
| `query_invocation`     | Query detail for a specific invocation by ID            |

## Design notes

WAL journal mode is set immediately after opening the connection. This is required for concurrent read/write access — without it, readers would block writers. The trade-off is a WAL file on disk alongside the database, but baton databases are small and short-lived enough that this is not a concern.

store_invocation uses UUIDs (v4) for primary keys rather than auto-increment integers. This avoids conflicts when merging history databases and makes IDs meaningful outside the database context (e.g., in logs or API responses).

---

## init_db

Opens or creates a SQLite database at the given path, sets WAL mode, and creates the schema (tables and indexes). Returns the open connection.

### Schema

The schema consists of three tables that mirror the runtime model:

- **invocations**: The top-level record of a `baton check` run. Records when, how, and in what environment the invocation happened.
- **validator_runs**: One row per execution of a stateless validator function. Records the gate, validator name, group key, every input file (path + content hash as JSON), verdict status, feedback, duration, and token usage.
- **gate_results**: Aggregated outcome of each gate. Records status, duration, validator count, and per-status breakdowns.

Schema creation is idempotent — safe to call on an existing database.

SPEC-HI-ID-001: open-connection-or-error
  When the SQLite connection cannot be opened (e.g., invalid path, permission denied), init_db returns Err(BatonError::DatabaseError) with a message containing "Failed to open database".
  test: UNTESTED (filesystem permission errors are platform-dependent and not exercised)

SPEC-HI-ID-002: wal-mode-set
  After opening the connection, init_db sets the journal_mode pragma to WAL. If the pragma update fails, it returns Err(BatonError::DatabaseError) with "Failed to set WAL mode".
  test: history::tests::wal_mode_enabled

SPEC-HI-ID-003: invocations-table-created
  init_db creates an invocations table that records the top-level invocation: a unique ID, when it happened, the CLI args that triggered it, git state, a hash of the config that was used, and the baton version. This is the anchor row that gate results and validator runs reference.
  test: history::tests::new_schema_invocations_table_exists

SPEC-HI-ID-004: validator-runs-table-created
  init_db creates a validator_runs table that records each execution of a stateless validator: which invocation it belongs to, which gate orchestrated it, the validator name, the group key (for multi-input validators), the verdict status, feedback text, execution duration, token usage (for LLM validators), and a JSON column recording every input file with its path and content hash.
  test: history::tests::new_schema_validator_runs_table_exists

SPEC-HI-ID-005: gate-results-table-created
  init_db creates a gate_results table that records the orchestration outcome of each gate: which invocation it belongs to, the gate name, overall status, duration, total validator count, and per-status counts (pass/fail/warn/error/skip). This is an aggregate — the individual details are in validator_runs.
  test: history::tests::new_schema_gate_results_table_exists

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

## store_invocation

Inserts an InvocationResult and its associated gate results and validator runs into the database. Returns the generated invocation UUID.

SPEC-HI-SI-001: invocation-id-is-uuid-v4
  store_invocation generates a UUID v4 string as the invocation's primary key. Each call produces a unique ID.
  test: history::tests::store_invocation_returns_uuid
  test: history::tests::store_invocation_unique_ids

SPEC-HI-SI-002: invocation-row-inserted
  store_invocation inserts one row into the invocations table. The timestamp is stored as an RFC 3339 string.
  test: history::tests::store_invocation_inserts_invocation_row

SPEC-HI-SI-003: gate-result-rows-inserted
  For each GateResult, store_invocation inserts one row into gate_results with aggregate counts (pass/fail/warn/error/skip).
  test: history::tests::store_invocation_inserts_gate_results

SPEC-HI-SI-004: validator-run-rows-inserted
  For each ValidatorResult within each gate, store_invocation inserts one row into validator_runs. The row includes the gate name, validator name, group_key, and input_files as a JSON column.
  test: history::tests::store_invocation_inserts_validator_runs

SPEC-HI-SI-005: input-files-stored-as-json
  The input_files column contains a JSON array of objects, each with `path` (string) and `hash` (string, SHA-256 of file content at time of validation).
  test: TODO (input_files JSON population depends on dispatch planner wiring)

SPEC-HI-SI-006: tokens-used-stored
  When a ValidatorResult has token usage data, the tokens_used column is populated. Otherwise it is NULL.
  test: history::tests::store_invocation_stores_tokens

SPEC-HI-SI-007: returns-invocation-id-on-success
  On success, store_invocation returns Ok(String) containing the UUID that was used as the invocation's primary key.
  test: history::tests::store_invocation_returns_uuid

---

## query_recent

Queries invocations with optional gate and/or status filters, limited to N results, ordered by timestamp descending (newest first). The query interface changes to reflect the new schema but the filtering semantics are similar.

SPEC-HI-QR-001: no-filters-returns-all
  When no filters are provided, query_recent returns all invocations (up to limit), ordered by timestamp DESC.
  test: TODO

SPEC-HI-QR-002: gate-filter
  When gate is provided, only invocations containing a gate_result matching that gate name are returned.
  test: TODO

SPEC-HI-QR-003: status-filter
  When status is provided, only invocations containing a gate_result with that status are returned.
  test: TODO

SPEC-HI-QR-004: ordered-by-timestamp-desc
  Results are always ordered by timestamp DESC (newest first).
  test: TODO

SPEC-HI-QR-005: limit-applied
  The LIMIT clause restricts the number of returned rows.
  test: TODO

SPEC-HI-QR-006: empty-db-returns-empty-vec
  When the database has no invocations, query_recent returns Ok(Vec::new()), not an error.
  test: TODO

---

## query_by_file

Queries validator runs by file path across the `validator_runs.input_files` JSON column.

SPEC-HI-QF-001: filters-by-file-path
  Searches for a file path within the JSON input_files array of validator_runs rows. Returns matching validator runs with their invocation context.
  test: TODO

SPEC-HI-QF-002: ordered-by-timestamp-desc
  Results are ordered by timestamp DESC.
  test: TODO

SPEC-HI-QF-003: no-match-returns-empty-vec
  When no validator runs reference the given path, returns Ok(Vec::new()).
  test: history::tests::query_by_file_no_match_returns_empty

---

## query_by_hash

Queries validator runs by content hash across the `validator_runs.input_files` JSON column.

SPEC-HI-QH-001: filters-by-content-hash
  Searches for a content hash within the JSON input_files array of validator_runs rows.
  test: TODO

SPEC-HI-QH-002: ordered-by-timestamp-desc
  Results are ordered by timestamp DESC.
  test: TODO

SPEC-HI-QH-003: no-match-returns-empty-vec
  When no validator runs have a file with the given hash, returns Ok(Vec::new()).
  test: history::tests::query_by_hash_no_match_returns_empty

---

## query_invocation

Queries detail for a specific invocation by ID, returning the full invocation with all gate results and validator runs.

SPEC-HI-QI-001: returns-full-invocation
  Given an invocation ID, returns the invocation row plus all associated gate_results and validator_runs.
  test: history::tests::query_invocation_returns_full_detail

SPEC-HI-QI-002: not-found-returns-error
  When the invocation ID does not exist, returns an error.
  test: history::tests::query_invocation_not_found_returns_error

---

## Concurrency

WAL mode enables concurrent readers and a single writer. The history module does not set busy_timeout — callers are responsible for configuring retry behavior when contention occurs.

SPEC-HI-CC-001: concurrent-writes-succeed-with-busy-timeout
  Multiple threads writing to the same database file via separate connections succeed when each connection has a busy_timeout configured. All writes are durable and queryable afterward.
  test: history::tests::concurrent_writes_via_separate_connections

SPEC-HI-CC-002: concurrent-read-write-succeeds
  A reader and writer operating concurrently via separate connections both succeed when busy_timeout is configured. The reader sees a consistent snapshot (at least the initial seed data). WAL mode ensures readers do not block writers.
  test: history::tests::concurrent_read_write
