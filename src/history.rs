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

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

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
    use crate::types::{Cost, Status, ValidatorResult, VerdictStatus};
    use chrono::Utc;
    use tempfile::TempDir;

    fn make_verdict(status: VerdictStatus) -> Verdict {
        Verdict {
            status,
            gate: "test-gate".into(),
            failed_at: if status != VerdictStatus::Pass {
                Some("lint".into())
            } else {
                None
            },
            feedback: if status != VerdictStatus::Pass {
                Some("something failed".into())
            } else {
                None
            },
            duration_ms: 100,
            timestamp: Utc::now(),
            artifact_hash: "abc123".into(),
            context_hash: "def456".into(),
            warnings: vec![],
            suppressed: vec![],
            history: vec![
                ValidatorResult {
                    name: "lint".into(),
                    status: if status == VerdictStatus::Pass {
                        Status::Pass
                    } else {
                        Status::Fail
                    },
                    feedback: None,
                    duration_ms: 50,
                    cost: Some(Cost {
                        input_tokens: Some(100),
                        output_tokens: Some(50),
                        model: Some("test-model".into()),
                        estimated_usd: Some(0.001),
                    }),
                },
            ],
        }
    }

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

        let verdict = make_verdict(VerdictStatus::Pass);
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

        let v1 = make_verdict(VerdictStatus::Pass);
        store_verdict(&conn, &v1).unwrap();

        let mut v2 = make_verdict(VerdictStatus::Fail);
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

        store_verdict(&conn, &make_verdict(VerdictStatus::Pass)).unwrap();
        store_verdict(&conn, &make_verdict(VerdictStatus::Fail)).unwrap();

        let results = query_recent(&conn, 10, None, Some("fail")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, "fail");
    }

    #[test]
    fn query_by_artifact_hash() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = init_db(&db_path).unwrap();

        store_verdict(&conn, &make_verdict(VerdictStatus::Pass)).unwrap();

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

        let verdict = make_verdict(VerdictStatus::Pass);
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
            store_verdict(&conn, &make_verdict(VerdictStatus::Pass)).unwrap();
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
        store_verdict(&conn1, &make_verdict(VerdictStatus::Pass)).unwrap();
        drop(conn1);

        let conn2 = init_db(&db_path).unwrap();
        let results = query_recent(&conn2, 10, None, None).unwrap();
        assert_eq!(results.len(), 1);
    }
}
