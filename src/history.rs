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
        CREATE INDEX IF NOT EXISTS idx_vresults_verdict ON validator_results(verdict_id);",
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
            verdict.artifact_hash,
            verdict.context_hash,
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
    fn init_db_creates_schema() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        // Verify tables exist
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='verdicts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='validator_results'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn store_and_query_verdict() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let verdict = th::verdict(VerdictStatus::Pass);
        let id = store_verdict(&conn, &verdict).unwrap();
        assert!(!id.is_empty());

        let results = query_recent(&conn, 10, None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].gate, "test-gate");
        assert_eq!(results[0].status, "pass");
    }

    #[test]
    fn query_by_gate() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let v1 = th::verdict(VerdictStatus::Pass);
        store_verdict(&conn, &v1).unwrap();

        let mut v2 = th::verdict(VerdictStatus::Fail);
        v2.gate = "other-gate".into();
        store_verdict(&conn, &v2).unwrap();

        let results = query_recent(&conn, 10, Some("test-gate"), None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].gate, "test-gate");
    }

    #[test]
    fn query_by_status() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        store_verdict(&conn, &th::verdict(VerdictStatus::Fail)).unwrap();

        let results = query_recent(&conn, 10, None, Some("fail")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, "fail");
    }

    #[test]
    fn query_by_artifact_hash() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();

        let results = query_by_artifact(&conn, "abc123").unwrap();
        assert_eq!(results.len(), 1);

        let results = query_by_artifact(&conn, "nonexistent").unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn store_verdict_with_cost() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let verdict = th::verdict(VerdictStatus::Pass);
        let verdict_id = store_verdict(&conn, &verdict).unwrap();

        // Verify cost data was stored
        let (input_tokens, output_tokens, model): (Option<i64>, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, model FROM validator_results WHERE verdict_id = ?1",
                params![verdict_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert_eq!(input_tokens, Some(100));
        assert_eq!(output_tokens, Some(50));
        assert_eq!(model, Some("test-model".into()));
    }

    #[test]
    fn query_limit() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        for _ in 0..5 {
            store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        }

        let results = query_recent(&conn, 3, None, None).unwrap();
        assert_eq!(results.len(), 3);
    }

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

    // ─── Empty DB queries ────────────────────────────

    #[test]
    fn query_recent_empty_db() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let results = query_recent(&conn, 10, None, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn query_by_artifact_empty_db() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let results = query_by_artifact(&conn, "abc123").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn query_recent_empty_db_with_limit_zero() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();

        let results = query_recent(&conn, 0, None, None).unwrap();
        assert!(results.is_empty());
    }

    // ─── Filters that match nothing ──────────────────

    #[test]
    fn query_by_gate_no_match() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();

        let results = query_recent(&conn, 10, Some("nonexistent-gate"), None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn query_by_status_no_match() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();

        let results = query_recent(&conn, 10, None, Some("error")).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn query_by_gate_and_status_no_match() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();

        // Gate matches but status doesn't
        let results = query_recent(&conn, 10, Some("test-gate"), Some("fail")).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn query_by_artifact_no_match() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();

        let results = query_by_artifact(&conn, "ffffffffffffffff").unwrap();
        assert!(results.is_empty());
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
                    for j in 0..writes_per_thread {
                        let mut v = th::verdict(VerdictStatus::Pass);
                        v.gate = format!("gate-{i}");
                        v.artifact_hash = format!("hash-{i}-{j}");
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

    // ─── Unique IDs ──────────────────────────────────

    #[test]
    fn store_verdict_returns_unique_ids() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let id1 = store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        let id2 = store_verdict(&conn, &th::verdict(VerdictStatus::Pass)).unwrap();
        assert_ne!(id1, id2);
        assert!(!id1.is_empty());
        assert!(!id2.is_empty());
    }

    // ─── Query ordering ─────────────────────────────

    #[test]
    fn query_recent_returns_newest_first() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        // Insert three verdicts with distinct timestamps
        for gate_name in &["first", "second", "third"] {
            let mut v = th::verdict(VerdictStatus::Pass);
            v.gate = gate_name.to_string();
            // Advance timestamp slightly to ensure ordering
            v.timestamp = chrono::Utc::now();
            store_verdict(&conn, &v).unwrap();
            // Sleep 10ms so timestamps are distinct
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let results = query_recent(&conn, 10, None, None).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].gate, "third");
        assert_eq!(results[1].gate, "second");
        assert_eq!(results[2].gate, "first");
    }

    // ─── Validator results FK linkage ────────────────

    #[test]
    fn validator_results_linked_to_verdict() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let verdict = th::verdict(VerdictStatus::Pass);
        let verdict_id = store_verdict(&conn, &verdict).unwrap();

        // Count validator_results rows linked to this verdict
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM validator_results WHERE verdict_id = ?1",
                params![verdict_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, verdict.history.len() as i64);
    }

    #[test]
    fn multiple_validator_results_stored() {
        use crate::types::{Cost, Status, ValidatorResult};

        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let mut v = th::verdict(VerdictStatus::Fail);
        v.history = vec![
            ValidatorResult {
                name: "lint".into(),
                status: Status::Pass,
                feedback: None,
                duration_ms: 10,
                cost: None,
            },
            ValidatorResult {
                name: "typecheck".into(),
                status: Status::Fail,
                feedback: Some("type error".into()),
                duration_ms: 20,
                cost: Some(Cost {
                    input_tokens: Some(500),
                    output_tokens: Some(100),
                    model: Some("test-model".into()),
                    estimated_usd: Some(0.005),
                }),
            },
            ValidatorResult {
                name: "format".into(),
                status: Status::Skip,
                feedback: None,
                duration_ms: 0,
                cost: None,
            },
        ];

        let verdict_id = store_verdict(&conn, &v).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM validator_results WHERE verdict_id = ?1",
                params![verdict_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);

        // Verify the typecheck row has cost data
        let (feedback, tokens): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT feedback, input_tokens FROM validator_results WHERE verdict_id = ?1 AND name = 'typecheck'",
                params![verdict_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(feedback, Some("type error".into()));
        assert_eq!(tokens, Some(500));
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

    // ─── verdict_json column ─────────────────────────

    #[test]
    fn verdict_json_column_is_parseable() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let verdict = th::verdict(VerdictStatus::Pass);
        let verdict_id = store_verdict(&conn, &verdict).unwrap();

        let json_str: String = conn
            .query_row(
                "SELECT verdict_json FROM verdicts WHERE id = ?1",
                params![verdict_id],
                |row| row.get(0),
            )
            .unwrap();

        // The stored JSON should deserialize back to a valid Verdict
        let parsed: crate::types::Verdict = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.status, VerdictStatus::Pass);
        assert_eq!(parsed.gate, "test-gate");
    }

    // ─── Spec coverage (UNTESTED) ──────────────────────

    #[test]
    fn index_existence_after_init_db() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'idx_%'")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        let expected = [
            "idx_verdicts_gate",
            "idx_verdicts_status",
            "idx_verdicts_artifact",
            "idx_verdicts_context",
            "idx_verdicts_timestamp",
            "idx_vresults_verdict",
        ];
        for idx in &expected {
            assert!(
                names.contains(&idx.to_string()),
                "missing index: {idx}, found: {names:?}"
            );
        }
        assert_eq!(names.len(), expected.len());
    }

    #[test]
    fn warnings_json_column_stores_warnings() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let mut v = th::verdict(VerdictStatus::Pass);
        v.warnings = vec!["warn1".into(), "warn2".into()];
        let verdict_id = store_verdict(&conn, &v).unwrap();

        let json_str: String = conn
            .query_row(
                "SELECT warnings_json FROM verdicts WHERE id = ?1",
                params![verdict_id],
                |row| row.get(0),
            )
            .unwrap();

        let parsed: Vec<String> = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, vec!["warn1", "warn2"]);
    }

    #[test]
    fn suppressed_json_column_stores_suppressed() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let mut v = th::verdict(VerdictStatus::Pass);
        v.suppressed = vec!["lint".into(), "format".into()];
        let verdict_id = store_verdict(&conn, &v).unwrap();

        let json_str: String = conn
            .query_row(
                "SELECT suppressed_json FROM verdicts WHERE id = ?1",
                params![verdict_id],
                |row| row.get(0),
            )
            .unwrap();

        let parsed: Vec<String> = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, vec!["lint", "format"]);
    }

    #[test]
    fn null_cost_columns_when_cost_is_none() {
        use crate::types::{Status, ValidatorResult};

        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let mut v = th::verdict(VerdictStatus::Pass);
        v.history = vec![ValidatorResult {
            name: "no-cost".into(),
            status: Status::Pass,
            feedback: None,
            duration_ms: 10,
            cost: None,
        }];
        let verdict_id = store_verdict(&conn, &v).unwrap();

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

        assert!(input_tokens.is_none());
        assert!(output_tokens.is_none());
        assert!(model.is_none());
        assert!(estimated_usd.is_none());
    }

    #[test]
    fn query_by_artifact_ordering_newest_first() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let shared_hash = "shared-artifact-hash";
        for gate_name in &["oldest", "middle", "newest"] {
            let mut v = th::verdict(VerdictStatus::Pass);
            v.artifact_hash = shared_hash.into();
            v.gate = gate_name.to_string();
            v.timestamp = chrono::Utc::now();
            store_verdict(&conn, &v).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let results = query_by_artifact(&conn, shared_hash).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].gate, "newest");
        assert_eq!(results[1].gate, "middle");
        assert_eq!(results[2].gate, "oldest");
    }

    #[test]
    fn query_by_artifact_returns_all_rows() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        let shared_hash = "all-rows-hash";
        for i in 0..5 {
            let mut v = th::verdict(VerdictStatus::Pass);
            v.artifact_hash = shared_hash.into();
            v.gate = format!("gate-{i}");
            store_verdict(&conn, &v).unwrap();
        }

        // Also insert a verdict with a different hash to ensure filtering works
        let mut other = th::verdict(VerdictStatus::Pass);
        other.artifact_hash = "other-hash".into();
        store_verdict(&conn, &other).unwrap();

        let results = query_by_artifact(&conn, shared_hash).unwrap();
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn verdict_summary_debug_and_clone() {
        let summary = VerdictSummary {
            id: "test-id".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            gate: "test-gate".into(),
            status: "pass".into(),
            failed_at: None,
            feedback: Some("all good".into()),
            duration_ms: 42,
            artifact_hash: "abc123".into(),
        };

        let _cloned = summary.clone();
        let _debug = format!("{:?}", summary);
    }
}
