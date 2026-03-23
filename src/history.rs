//! SQLite-based verdict history storage.
//!
//! Persists verdicts and individual validator results with indexes for
//! efficient querying by gate, status, artifact hash, and timestamp.

use rusqlite::{params, Connection};
use std::path::Path;

use crate::error::{BatonError, Result};
use crate::types::Verdict;

/// Initialize the history database with the required schema.
pub fn init_db(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)
        .map_err(|e| BatonError::DatabaseError(format!("Failed to open database: {e}")))?;

    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| BatonError::DatabaseError(format!("Failed to set busy timeout: {e}")))?;

    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| BatonError::DatabaseError(format!("Failed to set WAL mode: {e}")))?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS verdicts (
            id             TEXT PRIMARY KEY,
            timestamp      TEXT NOT NULL,
            gate           TEXT NOT NULL,
            status         TEXT NOT NULL,
            failed_at      TEXT,
            feedback       TEXT,
            duration_ms    INTEGER NOT NULL,
            artifact_hash  TEXT NOT NULL,
            context_hash   TEXT NOT NULL,
            warnings_json  TEXT,
            suppressed_json TEXT,
            verdict_json   TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS validator_results (
            id             TEXT PRIMARY KEY,
            verdict_id     TEXT NOT NULL REFERENCES verdicts(id),
            name           TEXT NOT NULL,
            status         TEXT NOT NULL,
            feedback       TEXT,
            duration_ms    INTEGER NOT NULL,
            input_tokens   INTEGER,
            output_tokens  INTEGER,
            model          TEXT,
            estimated_usd  REAL
        );
        CREATE INDEX IF NOT EXISTS idx_verdicts_gate ON verdicts(gate);
        CREATE INDEX IF NOT EXISTS idx_verdicts_status ON verdicts(status);
        CREATE INDEX IF NOT EXISTS idx_verdicts_artifact ON verdicts(artifact_hash);
        CREATE INDEX IF NOT EXISTS idx_verdicts_context ON verdicts(context_hash);
        CREATE INDEX IF NOT EXISTS idx_verdicts_timestamp ON verdicts(timestamp);
        CREATE INDEX IF NOT EXISTS idx_vresults_verdict ON validator_results(verdict_id);

        CREATE TABLE IF NOT EXISTS invocations (
            id             TEXT PRIMARY KEY,
            timestamp      TEXT NOT NULL,
            cli_args       TEXT,
            git_state      TEXT,
            config_hash    TEXT,
            baton_version  TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS validator_runs (
            id             TEXT PRIMARY KEY,
            invocation_id  TEXT NOT NULL REFERENCES invocations(id),
            gate           TEXT NOT NULL,
            validator      TEXT NOT NULL,
            group_key      TEXT,
            status         TEXT NOT NULL,
            feedback       TEXT,
            duration_ms    INTEGER NOT NULL,
            tokens_used    INTEGER,
            input_files    TEXT
        );
        CREATE TABLE IF NOT EXISTS gate_results (
            id             TEXT PRIMARY KEY,
            invocation_id  TEXT NOT NULL REFERENCES invocations(id),
            gate           TEXT NOT NULL,
            status         TEXT NOT NULL,
            duration_ms    INTEGER NOT NULL,
            validator_count INTEGER NOT NULL DEFAULT 0,
            pass_count     INTEGER NOT NULL DEFAULT 0,
            fail_count     INTEGER NOT NULL DEFAULT 0,
            warn_count     INTEGER NOT NULL DEFAULT 0,
            error_count    INTEGER NOT NULL DEFAULT 0,
            skip_count     INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_invocations_timestamp ON invocations(timestamp);
        CREATE INDEX IF NOT EXISTS idx_vruns_invocation ON validator_runs(invocation_id);
        CREATE INDEX IF NOT EXISTS idx_gresults_invocation ON gate_results(invocation_id);",
    )
    .map_err(|e| BatonError::DatabaseError(format!("Failed to create schema: {e}")))?;

    Ok(conn)
}

/// Store a verdict in the history database.
pub fn store_verdict(conn: &Connection, verdict: &Verdict) -> Result<String> {
    let verdict_id = uuid::Uuid::new_v4().to_string();
    let warnings_json = serde_json::to_string(&verdict.warnings).unwrap();
    let suppressed_json = serde_json::to_string(&verdict.suppressed).unwrap();
    let verdict_json = verdict.to_json();

    conn.execute(
        "INSERT INTO verdicts (id, timestamp, gate, status, failed_at, feedback, duration_ms, artifact_hash, context_hash, warnings_json, suppressed_json, verdict_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            verdict_id,
            verdict.timestamp.to_rfc3339(),
            verdict.gate,
            verdict.status.to_string(),
            verdict.failed_at,
            verdict.feedback,
            verdict.duration_ms,
            "",
            "",
            warnings_json,
            suppressed_json,
            verdict_json,
        ],
    )
    .map_err(|e| BatonError::DatabaseError(format!("Failed to insert verdict: {e}")))?;

    for result in &verdict.history {
        let result_id = uuid::Uuid::new_v4().to_string();
        let (input_tokens, output_tokens, model, estimated_usd) = match &result.cost {
            Some(cost) => (
                cost.input_tokens,
                cost.output_tokens,
                cost.model.clone(),
                cost.estimated_usd,
            ),
            None => (None, None, None, None),
        };

        conn.execute(
            "INSERT INTO validator_results (id, verdict_id, name, status, feedback, duration_ms, input_tokens, output_tokens, model, estimated_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                result_id,
                verdict_id,
                result.name,
                result.status.to_string(),
                result.feedback,
                result.duration_ms,
                input_tokens,
                output_tokens,
                model,
                estimated_usd,
            ],
        )
        .map_err(|e| {
            BatonError::DatabaseError(format!("Failed to insert validator result: {e}"))
        })?;
    }

    Ok(verdict_id)
}

/// Query recent verdicts from the history database.
pub fn query_recent(
    conn: &Connection,
    limit: usize,
    gate: Option<&str>,
    status: Option<&str>,
) -> Result<Vec<VerdictSummary>> {
    let mut sql = "SELECT id, timestamp, gate, status, failed_at, feedback, duration_ms, artifact_hash FROM verdicts".to_string();
    let mut conditions = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(g) = gate {
        conditions.push("gate = ?");
        param_values.push(Box::new(g.to_string()));
    }
    if let Some(s) = status {
        conditions.push("status = ?");
        param_values.push(Box::new(s.to_string()));
    }
    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }
    sql.push_str(" ORDER BY timestamp DESC LIMIT ?");
    param_values.push(Box::new(limit as i64));

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| BatonError::DatabaseError(format!("Query error: {e}")))?;

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let rows = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(VerdictSummary {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                gate: row.get(2)?,
                status: row.get(3)?,
                failed_at: row.get(4)?,
                feedback: row.get(5)?,
                duration_ms: row.get(6)?,
                artifact_hash: row.get(7)?,
            })
        })
        .map_err(|e| BatonError::DatabaseError(format!("Query error: {e}")))?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| BatonError::DatabaseError(format!("Row error: {e}")))?);
    }

    Ok(results)
}

/// Query verdicts for a specific artifact hash.
pub fn query_by_artifact(conn: &Connection, artifact_hash: &str) -> Result<Vec<VerdictSummary>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, gate, status, failed_at, feedback, duration_ms, artifact_hash
             FROM verdicts WHERE artifact_hash = ?1 ORDER BY timestamp DESC",
        )
        .map_err(|e| BatonError::DatabaseError(format!("Query error: {e}")))?;

    let rows = stmt
        .query_map(params![artifact_hash], |row| {
            Ok(VerdictSummary {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                gate: row.get(2)?,
                status: row.get(3)?,
                failed_at: row.get(4)?,
                feedback: row.get(5)?,
                duration_ms: row.get(6)?,
                artifact_hash: row.get(7)?,
            })
        })
        .map_err(|e| BatonError::DatabaseError(format!("Query error: {e}")))?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| BatonError::DatabaseError(format!("Row error: {e}")))?);
    }

    Ok(results)
}

/// Summary row for history queries.
#[derive(Debug, Clone)]
pub struct VerdictSummary {
    pub id: String,
    pub timestamp: String,
    pub gate: String,
    pub status: String,
    pub failed_at: Option<String>,
    pub feedback: Option<String>,
    pub duration_ms: i64,
    pub artifact_hash: String,
}

// ─── v2 history functions ────────────────────────────────

use crate::types::{InvocationResult, Status, ValidatorResult};

/// Store an invocation result in the v2 history schema.
///
/// Inserts one row into `invocations`, one per gate into `gate_results`,
/// and one per validator run into `validator_runs`. Returns the generated UUID.
pub fn store_invocation(conn: &Connection, result: &InvocationResult) -> Result<String> {
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().to_rfc3339();
    let baton_version = env!("CARGO_PKG_VERSION");

    conn.execute(
        "INSERT INTO invocations (id, timestamp, cli_args, git_state, config_hash, baton_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![invocation_id, timestamp, None::<String>, None::<String>, None::<String>, baton_version],
    )
    .map_err(|e| BatonError::DatabaseError(format!("Failed to insert invocation: {e}")))?;

    for gate_result in &result.gate_results {
        let gate_id = uuid::Uuid::new_v4().to_string();
        let (pass_count, fail_count, warn_count, error_count, skip_count) =
            count_statuses(&gate_result.validator_results);
        let validator_count = gate_result.validator_results.len() as i64;

        conn.execute(
            "INSERT INTO gate_results (id, invocation_id, gate, status, duration_ms, validator_count, pass_count, fail_count, warn_count, error_count, skip_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                gate_id,
                invocation_id,
                gate_result.gate_name,
                gate_result.status.to_string(),
                gate_result.duration.as_millis() as i64,
                validator_count,
                pass_count,
                fail_count,
                warn_count,
                error_count,
                skip_count,
            ],
        )
        .map_err(|e| BatonError::DatabaseError(format!("Failed to insert gate result: {e}")))?;

        for vr in &gate_result.validator_results {
            let run_id = uuid::Uuid::new_v4().to_string();
            let tokens_used = vr
                .cost
                .as_ref()
                .and_then(|c| c.input_tokens.map(|i| i + c.output_tokens.unwrap_or(0)));

            conn.execute(
                "INSERT INTO validator_runs (id, invocation_id, gate, validator, group_key, status, feedback, duration_ms, tokens_used, input_files)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    run_id,
                    invocation_id,
                    gate_result.gate_name,
                    vr.name,
                    None::<String>, // group_key — not yet populated from Invocation
                    vr.status.to_string(),
                    vr.feedback,
                    vr.duration_ms,
                    tokens_used,
                    None::<String>, // input_files JSON — populated when dispatch planner is wired
                ],
            )
            .map_err(|e| {
                BatonError::DatabaseError(format!("Failed to insert validator run: {e}"))
            })?;
        }
    }

    Ok(invocation_id)
}

fn count_statuses(results: &[ValidatorResult]) -> (i64, i64, i64, i64, i64) {
    let mut pass = 0i64;
    let mut fail = 0i64;
    let mut warn = 0i64;
    let mut error = 0i64;
    let mut skip = 0i64;
    for r in results {
        match r.status {
            Status::Pass => pass += 1,
            Status::Fail => fail += 1,
            Status::Warn => warn += 1,
            Status::Error => error += 1,
            Status::Skip => skip += 1,
        }
    }
    (pass, fail, warn, error, skip)
}

/// Summary row for v2 validator run queries.
#[derive(Debug, Clone)]
pub struct ValidatorRunSummary {
    pub id: String,
    pub invocation_id: String,
    pub gate: String,
    pub validator: String,
    pub group_key: Option<String>,
    pub status: String,
    pub feedback: Option<String>,
    pub duration_ms: i64,
    pub input_files: Option<String>,
    pub timestamp: String,
}

/// Full invocation detail with nested gate results and validator runs.
#[derive(Debug, Clone)]
pub struct InvocationDetail {
    pub id: String,
    pub timestamp: String,
    pub baton_version: String,
    pub gate_results: Vec<GateResultDetail>,
    pub validator_runs: Vec<ValidatorRunSummary>,
}

/// Gate result detail row.
#[derive(Debug, Clone)]
pub struct GateResultDetail {
    pub id: String,
    pub gate: String,
    pub status: String,
    pub duration_ms: i64,
    pub validator_count: i64,
    pub pass_count: i64,
    pub fail_count: i64,
    pub warn_count: i64,
    pub error_count: i64,
    pub skip_count: i64,
}

/// Query validator runs by file path (searches input_files JSON).
pub fn query_by_file(conn: &Connection, file_path: &str) -> Result<Vec<ValidatorRunSummary>> {
    let pattern = format!("%\"path\":\"{}\"%" , file_path.replace('"', "\\\""));
    let mut stmt = conn
        .prepare(
            "SELECT vr.id, vr.invocation_id, vr.gate, vr.validator, vr.group_key, vr.status, vr.feedback, vr.duration_ms, vr.input_files, i.timestamp
             FROM validator_runs vr
             JOIN invocations i ON i.id = vr.invocation_id
             WHERE vr.input_files LIKE ?1
             ORDER BY i.timestamp DESC",
        )
        .map_err(|e| BatonError::DatabaseError(format!("Query error: {e}")))?;

    let rows = stmt
        .query_map(params![pattern], |row| {
            Ok(ValidatorRunSummary {
                id: row.get(0)?,
                invocation_id: row.get(1)?,
                gate: row.get(2)?,
                validator: row.get(3)?,
                group_key: row.get(4)?,
                status: row.get(5)?,
                feedback: row.get(6)?,
                duration_ms: row.get(7)?,
                input_files: row.get(8)?,
                timestamp: row.get(9)?,
            })
        })
        .map_err(|e| BatonError::DatabaseError(format!("Query error: {e}")))?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| BatonError::DatabaseError(format!("Row error: {e}")))?);
    }
    Ok(results)
}

/// Query validator runs by content hash (searches input_files JSON).
pub fn query_by_hash(conn: &Connection, hash: &str) -> Result<Vec<ValidatorRunSummary>> {
    let pattern = format!("%\"hash\":\"{}\"%" , hash);
    let mut stmt = conn
        .prepare(
            "SELECT vr.id, vr.invocation_id, vr.gate, vr.validator, vr.group_key, vr.status, vr.feedback, vr.duration_ms, vr.input_files, i.timestamp
             FROM validator_runs vr
             JOIN invocations i ON i.id = vr.invocation_id
             WHERE vr.input_files LIKE ?1
             ORDER BY i.timestamp DESC",
        )
        .map_err(|e| BatonError::DatabaseError(format!("Query error: {e}")))?;

    let rows = stmt
        .query_map(params![pattern], |row| {
            Ok(ValidatorRunSummary {
                id: row.get(0)?,
                invocation_id: row.get(1)?,
                gate: row.get(2)?,
                validator: row.get(3)?,
                group_key: row.get(4)?,
                status: row.get(5)?,
                feedback: row.get(6)?,
                duration_ms: row.get(7)?,
                input_files: row.get(8)?,
                timestamp: row.get(9)?,
            })
        })
        .map_err(|e| BatonError::DatabaseError(format!("Query error: {e}")))?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| BatonError::DatabaseError(format!("Row error: {e}")))?);
    }
    Ok(results)
}

/// Query full invocation detail by ID.
pub fn query_invocation(conn: &Connection, id: &str) -> Result<InvocationDetail> {
    // Get invocation row
    let (timestamp, baton_version): (String, String) = conn
        .query_row(
            "SELECT timestamp, baton_version FROM invocations WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                BatonError::DatabaseError(format!("Invocation not found: {id}"))
            }
            _ => BatonError::DatabaseError(format!("Query error: {e}")),
        })?;

    // Get gate results
    let mut gate_stmt = conn
        .prepare(
            "SELECT id, gate, status, duration_ms, validator_count, pass_count, fail_count, warn_count, error_count, skip_count
             FROM gate_results WHERE invocation_id = ?1",
        )
        .map_err(|e| BatonError::DatabaseError(format!("Query error: {e}")))?;

    let gate_results: Vec<GateResultDetail> = gate_stmt
        .query_map(params![id], |row| {
            Ok(GateResultDetail {
                id: row.get(0)?,
                gate: row.get(1)?,
                status: row.get(2)?,
                duration_ms: row.get(3)?,
                validator_count: row.get(4)?,
                pass_count: row.get(5)?,
                fail_count: row.get(6)?,
                warn_count: row.get(7)?,
                error_count: row.get(8)?,
                skip_count: row.get(9)?,
            })
        })
        .map_err(|e| BatonError::DatabaseError(format!("Query error: {e}")))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| BatonError::DatabaseError(format!("Row error: {e}")))?;

    // Get validator runs
    let mut vr_stmt = conn
        .prepare(
            "SELECT id, invocation_id, gate, validator, group_key, status, feedback, duration_ms, input_files
             FROM validator_runs WHERE invocation_id = ?1",
        )
        .map_err(|e| BatonError::DatabaseError(format!("Query error: {e}")))?;

    let validator_runs: Vec<ValidatorRunSummary> = vr_stmt
        .query_map(params![id], |row| {
            Ok(ValidatorRunSummary {
                id: row.get(0)?,
                invocation_id: row.get(1)?,
                gate: row.get(2)?,
                validator: row.get(3)?,
                group_key: row.get(4)?,
                status: row.get(5)?,
                feedback: row.get(6)?,
                duration_ms: row.get(7)?,
                input_files: row.get(8)?,
                timestamp: timestamp.clone(),
            })
        })
        .map_err(|e| BatonError::DatabaseError(format!("Query error: {e}")))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| BatonError::DatabaseError(format!("Row error: {e}")))?;

    Ok(InvocationDetail {
        id: id.to_string(),
        timestamp,
        baton_version,
        gate_results,
        validator_runs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers as th;
    use crate::types::VerdictStatus;
    use tempfile::TempDir;

    // ═══════════════════════════════════════════════════════════════
    // Behavioral contract tests
    // (all tests in this module exercise public API: init_db,
    //  store_verdict, query_recent, query_by_artifact)
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn idempotent_init() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // Init twice should work
        let conn1 = init_db(&db_path).unwrap();
        store_verdict(&conn1, &th::verdict(VerdictStatus::Pass)).unwrap();
        drop(conn1);

        let conn2 = init_db(&db_path).unwrap();
        let results = query_recent(&conn2, 10, None, None).unwrap();
        assert_eq!(results.len(), 1);
    }

    // ─── Concurrent writes ───────────────────────────

    #[test]
    fn concurrent_writes_via_separate_connections() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // Init the DB once
        let conn = init_db(&db_path).unwrap();
        drop(conn);

        let n_threads = 8;
        let writes_per_thread = 10;

        let handles: Vec<_> = (0..n_threads)
            .map(|i| {
                let path = db_path.clone();
                std::thread::spawn(move || {
                    let conn = init_db(&path).unwrap();
                    for _j in 0..writes_per_thread {
                        let mut v = th::verdict(VerdictStatus::Pass);
                        v.gate = format!("gate-{i}");
                        store_verdict(&conn, &v).unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // Verify all writes landed
        let conn = init_db(&db_path).unwrap();
        let results = query_recent(&conn, 1000, None, None).unwrap();
        assert_eq!(results.len(), n_threads * writes_per_thread);
    }

    #[test]
    fn concurrent_read_write() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = init_db(&db_path).unwrap();
        // Seed with some data
        for _ in 0..5 {
            store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        }
        drop(conn);

        let writer_path = db_path.clone();
        let reader_path = db_path.clone();

        let writer = std::thread::spawn(move || {
            let conn = init_db(&writer_path).unwrap();
            for _ in 0..20 {
                store_verdict(&conn, &th::verdict(VerdictStatus::Fail)).unwrap();
            }
        });

        let reader = std::thread::spawn(move || {
            let conn = init_db(&reader_path).unwrap();
            let mut total_seen = 0;
            for _ in 0..20 {
                let results = query_recent(&conn, 1000, None, None).unwrap();
                total_seen = total_seen.max(results.len());
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            // Reader should always see a consistent snapshot (>= 5 initial rows)
            assert!(total_seen >= 5);
        });

        writer.join().unwrap();
        reader.join().unwrap();

        // Final count should be 25
        let conn = init_db(&db_path).unwrap();
        let results = query_recent(&conn, 1000, None, None).unwrap();
        assert_eq!(results.len(), 25);
    }

    // ─── DB corruption recovery ──────────────────────

    #[test]
    fn corrupted_db_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // Write garbage to the file
        std::fs::write(&db_path, b"this is not a sqlite database at all").unwrap();

        let result = init_db(&db_path);
        assert!(result.is_err());
    }

    #[test]
    fn truncated_db_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // Create a valid DB, then truncate it
        let conn = init_db(&db_path).unwrap();
        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        drop(conn);

        // Truncate to a few bytes — corrupts the header
        std::fs::write(&db_path, &b"SQLite format 3\0"[..10]).unwrap();

        let result = init_db(&db_path);
        assert!(result.is_err());
    }

    #[test]
    fn missing_tables_after_manual_drop() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = init_db(&db_path).unwrap();
        // Manually drop the table
        conn.execute_batch("DROP TABLE validator_results; DROP TABLE verdicts;")
            .unwrap();
        drop(conn);

        // Re-init should recreate the schema (CREATE IF NOT EXISTS)
        let conn = init_db(&db_path).unwrap();
        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        let results = query_recent(&conn, 10, None, None).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn zero_byte_db_file() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // Create a zero-byte file — SQLite treats this as a new database
        std::fs::write(&db_path, b"").unwrap();

        let conn = init_db(&db_path).unwrap();
        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        let results = query_recent(&conn, 10, None, None).unwrap();
        assert_eq!(results.len(), 1);
    }

    // ─── WAL mode ────────────────────────────────────

    #[test]
    fn wal_mode_enabled() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    // ═══════════════════════════════════════════════════════════════
    // v2 migration: New schema tests (SPEC-HI-ID-003/004/005)
    // These tests verify the new three-table schema. They will fail
    // until the schema migration from verdicts/validator_results to
    // invocations/gate_results/validator_runs is implemented.
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn new_schema_invocations_table_exists() {
        // SPEC-HI-ID-003: invocations table with expected columns
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        // Verify table exists by querying its columns via pragma
        let mut stmt = conn.prepare("PRAGMA table_info(invocations)").unwrap();
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(columns.contains(&"id".to_string()), "Missing 'id' column");
        assert!(
            columns.contains(&"timestamp".to_string()),
            "Missing 'timestamp' column"
        );
        assert!(
            columns.contains(&"baton_version".to_string()),
            "Missing 'baton_version' column"
        );
    }

    #[test]
    fn new_schema_validator_runs_table_exists() {
        // SPEC-HI-ID-004: validator_runs table with expected columns
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let mut stmt = conn.prepare("PRAGMA table_info(validator_runs)").unwrap();
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(columns.contains(&"id".to_string()), "Missing 'id'");
        assert!(
            columns.contains(&"invocation_id".to_string()),
            "Missing 'invocation_id'"
        );
        assert!(columns.contains(&"gate".to_string()), "Missing 'gate'");
        assert!(
            columns.contains(&"validator".to_string()),
            "Missing 'validator'"
        );
        assert!(
            columns.contains(&"group_key".to_string()),
            "Missing 'group_key'"
        );
        assert!(columns.contains(&"status".to_string()), "Missing 'status'");
        assert!(
            columns.contains(&"input_files".to_string()),
            "Missing 'input_files'"
        );
    }

    #[test]
    fn new_schema_gate_results_table_exists() {
        // SPEC-HI-ID-005: gate_results table with expected columns
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let mut stmt = conn.prepare("PRAGMA table_info(gate_results)").unwrap();
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(columns.contains(&"id".to_string()), "Missing 'id'");
        assert!(
            columns.contains(&"invocation_id".to_string()),
            "Missing 'invocation_id'"
        );
        assert!(columns.contains(&"gate".to_string()), "Missing 'gate'");
        assert!(columns.contains(&"status".to_string()), "Missing 'status'");
        assert!(
            columns.contains(&"validator_count".to_string()),
            "Missing 'validator_count'"
        );
        assert!(
            columns.contains(&"pass_count".to_string()),
            "Missing 'pass_count'"
        );
        assert!(
            columns.contains(&"fail_count".to_string()),
            "Missing 'fail_count'"
        );
    }

    // ─── store_verdict ─────────────────────────────────

    #[test]
    fn store_verdict_returns_uuid() {
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let id = store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        assert!(!id.is_empty());
        // UUID v4 format: 8-4-4-4-12 hex digits
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().filter(|c| *c == '-').count(), 4);
    }

    #[test]
    fn store_verdict_stores_all_fields() {
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let verdict = th::verdict(VerdictStatus::Fail);
        store_verdict(&conn, &verdict).unwrap();

        let results = query_recent(&conn, 10, None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].gate, "test-gate");
        assert_eq!(results[0].status, "fail");
        assert_eq!(results[0].failed_at, Some("lint".into()));
        assert_eq!(results[0].feedback, Some("something failed".into()));
        assert_eq!(results[0].duration_ms, 100);
    }

    #[test]
    fn store_verdict_also_inserts_validator_results() {
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let verdict_id = store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM validator_results WHERE verdict_id = ?1",
                params![verdict_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn store_verdict_persists_cost_metadata() {
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let verdict_id = store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();

        let (input_tokens, output_tokens, model, estimated_usd): (
            Option<i64>,
            Option<i64>,
            Option<String>,
            Option<f64>,
        ) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, model, estimated_usd FROM validator_results WHERE verdict_id = ?1",
                params![verdict_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();

        assert_eq!(input_tokens, Some(100));
        assert_eq!(output_tokens, Some(50));
        assert_eq!(model, Some("test-model".into()));
        assert!((estimated_usd.unwrap() - 0.001).abs() < 0.0001);
    }

    #[test]
    fn store_multiple_verdicts_generates_unique_ids() {
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let id1 = store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        let id2 = store_verdict(&conn, &th::verdict(VerdictStatus::Fail)).unwrap();
        assert_ne!(id1, id2);
    }

    // ─── query_recent ────────────────────────────────────

    #[test]
    fn query_recent_empty_db() {
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let results = query_recent(&conn, 10, None, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn query_recent_respects_limit() {
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        for _ in 0..5 {
            store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        }
        let results = query_recent(&conn, 3, None, None).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn query_recent_filters_by_gate() {
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();

        // test-gate is the default from th::verdict
        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        let mut other = th::verdict(VerdictStatus::Pass);
        other.gate = "other-gate".into();
        store_verdict(&conn, &other).unwrap();

        let results = query_recent(&conn, 10, Some("test-gate"), None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].gate, "test-gate");
    }

    #[test]
    fn query_recent_filters_by_status() {
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();

        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        store_verdict(&conn, &th::verdict(VerdictStatus::Fail)).unwrap();

        let results = query_recent(&conn, 10, None, Some("pass")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, "pass");
    }

    #[test]
    fn query_recent_filters_by_gate_and_status() {
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();

        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        store_verdict(&conn, &th::verdict(VerdictStatus::Fail)).unwrap();
        let mut other = th::verdict(VerdictStatus::Pass);
        other.gate = "other-gate".into();
        store_verdict(&conn, &other).unwrap();

        let results = query_recent(&conn, 10, Some("test-gate"), Some("pass")).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn query_recent_orders_by_timestamp_desc() {
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();

        for _ in 0..3 {
            store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let results = query_recent(&conn, 10, None, None).unwrap();
        assert_eq!(results.len(), 3);
        // First result should be the most recent
        assert!(results[0].timestamp >= results[1].timestamp);
        assert!(results[1].timestamp >= results[2].timestamp);
    }

    // ═══════════════════════════════════════════════════════════════
    // v2 store_invocation tests (SPEC-HI-SI-*)
    // ═══════════════════════════════════════════════════════════════

    use crate::types::{Cost, GateResult, InvocationResult, Status};

    fn test_invocation_result() -> InvocationResult {
        InvocationResult {
            id: String::new(),
            gate_results: vec![GateResult {
                gate_name: "pre-commit".into(),
                status: Status::Pass,
                validator_results: vec![
                    crate::types::ValidatorResult {
                        name: "lint".into(),
                        status: Status::Pass,
                        feedback: None,
                        duration_ms: 50,
                        cost: None,
                    },
                    crate::types::ValidatorResult {
                        name: "typecheck".into(),
                        status: Status::Fail,
                        feedback: Some("type error".into()),
                        duration_ms: 120,
                        cost: Some(Cost {
                            input_tokens: Some(500),
                            output_tokens: Some(200),
                            model: Some("gpt-4".into()),
                            estimated_usd: Some(0.01),
                        }),
                    },
                ],
                duration: std::time::Duration::from_millis(170),
            }],
            duration: std::time::Duration::from_millis(170),
        }
    }

    #[test]
    fn store_invocation_returns_uuid() {
        // SPEC-HI-SI-001: invocation-id-is-uuid-v4
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let id = store_invocation(&conn, &test_invocation_result()).unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().filter(|c| *c == '-').count(), 4);
    }

    #[test]
    fn store_invocation_unique_ids() {
        // SPEC-HI-SI-001: each call produces a unique ID
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let id1 = store_invocation(&conn, &test_invocation_result()).unwrap();
        let id2 = store_invocation(&conn, &test_invocation_result()).unwrap();
        assert_ne!(id1, id2);
    }

    #[test]
    fn store_invocation_inserts_invocation_row() {
        // SPEC-HI-SI-002: invocation-row-inserted
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let id = store_invocation(&conn, &test_invocation_result()).unwrap();

        let (timestamp, version): (String, String) = conn
            .query_row(
                "SELECT timestamp, baton_version FROM invocations WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(!timestamp.is_empty());
        assert!(!version.is_empty());
    }

    #[test]
    fn store_invocation_inserts_gate_results() {
        // SPEC-HI-SI-003: gate-result-rows-inserted
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let id = store_invocation(&conn, &test_invocation_result()).unwrap();

        let (gate, status, vc, pc, fc): (String, String, i64, i64, i64) = conn
            .query_row(
                "SELECT gate, status, validator_count, pass_count, fail_count FROM gate_results WHERE invocation_id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(gate, "pre-commit");
        assert_eq!(status, "pass");
        assert_eq!(vc, 2);
        assert_eq!(pc, 1);
        assert_eq!(fc, 1);
    }

    #[test]
    fn store_invocation_inserts_validator_runs() {
        // SPEC-HI-SI-004: validator-run-rows-inserted
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let id = store_invocation(&conn, &test_invocation_result()).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM validator_runs WHERE invocation_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn store_invocation_stores_tokens() {
        // SPEC-HI-SI-006: tokens-used-stored
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let id = store_invocation(&conn, &test_invocation_result()).unwrap();

        let tokens: Option<i64> = conn
            .query_row(
                "SELECT tokens_used FROM validator_runs WHERE invocation_id = ?1 AND validator = 'typecheck'",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tokens, Some(700)); // 500 input + 200 output
    }

    // ─── query_by_file tests (SPEC-HI-QF-*) ───────────────

    #[test]
    fn query_by_file_no_match_returns_empty() {
        // SPEC-HI-QF-003
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        store_invocation(&conn, &test_invocation_result()).unwrap();

        let results = query_by_file(&conn, "/nonexistent/file.py").unwrap();
        assert!(results.is_empty());
    }

    // ─── query_by_hash tests (SPEC-HI-QH-*) ───────────────

    #[test]
    fn query_by_hash_no_match_returns_empty() {
        // SPEC-HI-QH-003
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        store_invocation(&conn, &test_invocation_result()).unwrap();

        let results = query_by_hash(&conn, "deadbeef").unwrap();
        assert!(results.is_empty());
    }

    // ─── query_invocation tests (SPEC-HI-QI-*) ────────────

    #[test]
    fn query_invocation_returns_full_detail() {
        // SPEC-HI-QI-001
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();
        let id = store_invocation(&conn, &test_invocation_result()).unwrap();

        let detail = query_invocation(&conn, &id).unwrap();
        assert_eq!(detail.id, id);
        assert_eq!(detail.gate_results.len(), 1);
        assert_eq!(detail.gate_results[0].gate, "pre-commit");
        assert_eq!(detail.validator_runs.len(), 2);
    }

    #[test]
    fn query_invocation_not_found_returns_error() {
        // SPEC-HI-QI-002
        let dir = TempDir::new().unwrap();
        let conn = init_db(&dir.path().join("test.db")).unwrap();

        let result = query_invocation(&conn, "nonexistent-id");
        assert!(result.is_err());
    }
}
