//! Verification storage for task quality gates
//!
//! Stores verification results in SQLite. Verifications are created when
//! attempting to close a task, with a Haiku subagent reviewing the work.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::error::StoreError;
use cas_types::{
    IssueSeverity, Verification, VerificationIssue, VerificationStatus, VerificationType,
};

// Helper to convert lock errors
fn lock_err<T>(_: std::sync::PoisonError<T>) -> StoreError {
    StoreError::Parse("Failed to acquire lock".to_string())
}

/// SQLite DDL for the `verifications` and `verification_issues` tables.
///
/// Re-exported via `cas_store::VERIFICATION_SCHEMA` so the migration runner in
/// `cas-cli` can bootstrap the base tables before applying ALTER migrations.
/// See cas-bdb9 / EPIC cas-9fdb.
pub const VERIFICATION_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS verifications (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    agent_id TEXT,
    verification_type TEXT NOT NULL DEFAULT 'task',
    status TEXT NOT NULL DEFAULT 'approved',
    confidence REAL,
    summary TEXT NOT NULL DEFAULT '',
    files_reviewed TEXT NOT NULL DEFAULT '[]',
    duration_ms INTEGER,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS verification_issues (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    verification_id TEXT NOT NULL,
    file TEXT NOT NULL,
    line INTEGER,
    severity TEXT NOT NULL DEFAULT 'blocking',
    category TEXT NOT NULL,
    code TEXT NOT NULL DEFAULT '',
    problem TEXT NOT NULL,
    suggestion TEXT,
    FOREIGN KEY (verification_id) REFERENCES verifications(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_verifications_task ON verifications(task_id);
CREATE INDEX IF NOT EXISTS idx_verifications_status ON verifications(status);
CREATE INDEX IF NOT EXISTS idx_verifications_created ON verifications(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_verification_issues_verification ON verification_issues(verification_id);
"#;

/// Trait for verification storage operations
pub trait VerificationStore: Send + Sync {
    /// Initialize the store (create tables)
    fn init(&self) -> Result<()>;

    /// Generate a new unique verification ID (e.g., ver-a1b2)
    fn generate_id(&self) -> Result<String>;

    /// Add a new verification with its issues
    fn add(&self, verification: &Verification) -> Result<()>;

    /// Get a verification by ID (includes issues)
    fn get(&self, id: &str) -> Result<Verification>;

    /// Update an existing verification
    fn update(&self, verification: &Verification) -> Result<()>;

    /// Delete a verification and its issues
    fn delete(&self, id: &str) -> Result<()>;

    /// Get verifications for a task
    fn get_for_task(&self, task_id: &str) -> Result<Vec<Verification>>;

    /// Get the most recent verification for a task
    fn get_latest_for_task(&self, task_id: &str) -> Result<Option<Verification>>;

    /// Get the most recent verification for a task of a specific type
    fn get_latest_for_task_by_type(
        &self,
        task_id: &str,
        verification_type: VerificationType,
    ) -> Result<Option<Verification>>;

    /// List recent verifications
    fn list_recent(&self, limit: usize) -> Result<Vec<Verification>>;

    /// List verifications by status
    fn list_by_status(&self, status: VerificationStatus) -> Result<Vec<Verification>>;

    /// Delete verifications older than the given number of days
    fn prune(&self, older_than_days: i64) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite-based verification store
pub struct SqliteVerificationStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteVerificationStore {
    /// Open or create a SQLite verification store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        let store = Self { conn };

        store.init()?;
        Ok(store)
    }

    fn parse_verification(row: &rusqlite::Row) -> rusqlite::Result<Verification> {
        let verification_type_str: String = row.get(3)?;
        let verification_type =
            VerificationType::from_str(&verification_type_str).unwrap_or_default();

        let status_str: String = row.get(4)?;
        let status = VerificationStatus::from_str(&status_str).unwrap_or_default();

        let created_at_str: String = row.get(9)?;
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        let files_reviewed_json: String = row.get(7)?;
        let files_reviewed: Vec<String> =
            serde_json::from_str(&files_reviewed_json).unwrap_or_default();

        Ok(Verification {
            id: row.get(0)?,
            task_id: row.get(1)?,
            agent_id: row.get(2)?,
            verification_type,
            status,
            confidence: row.get(5)?,
            summary: row.get(6)?,
            files_reviewed,
            duration_ms: row.get::<_, Option<i64>>(8)?.map(|v| v as u64),
            created_at,
            issues: Vec::new(), // Issues loaded separately
        })
    }

    fn load_issues(
        &self,
        conn: &Connection,
        verification_id: &str,
    ) -> Result<Vec<VerificationIssue>> {
        let mut stmt = conn.prepare_cached(
            "SELECT file, line, severity, category, code, problem, suggestion
             FROM verification_issues WHERE verification_id = ?1
             ORDER BY id",
        )?;

        let issues = stmt
            .query_map(params![verification_id], |row| {
                let severity_str: String = row.get(2)?;
                let severity = IssueSeverity::from_str(&severity_str).unwrap_or_default();

                Ok(VerificationIssue {
                    file: row.get(0)?,
                    line: row.get::<_, Option<i32>>(1)?.map(|v| v as u32),
                    severity,
                    category: row.get(3)?,
                    code: row.get(4)?,
                    problem: row.get(5)?,
                    suggestion: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(issues)
    }

    fn load_issues_batch(
        &self,
        conn: &Connection,
        verification_ids: &[&str],
    ) -> Result<HashMap<String, Vec<VerificationIssue>>> {
        if verification_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders: Vec<String> = (0..verification_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let query = format!(
            "SELECT verification_id, file, line, severity, category, code, problem, suggestion
             FROM verification_issues WHERE verification_id IN ({})
             ORDER BY id",
            placeholders.join(", ")
        );

        let mut stmt = conn.prepare(&query)?;
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(verification_ids.len());
        for id in verification_ids {
            params_vec.push(id);
        }

        let rows = stmt.query_map(params_vec.as_slice(), |row| {
            let vid: String = row.get(0)?;
            let severity_str: String = row.get(3)?;
            let severity = IssueSeverity::from_str(&severity_str).unwrap_or_default();

            Ok((
                vid,
                VerificationIssue {
                    file: row.get(1)?,
                    line: row.get::<_, Option<i32>>(2)?.map(|v| v as u32),
                    severity,
                    category: row.get(4)?,
                    code: row.get(5)?,
                    problem: row.get(6)?,
                    suggestion: row.get(7)?,
                },
            ))
        })?;

        let mut map: HashMap<String, Vec<VerificationIssue>> = HashMap::new();
        for row in rows.filter_map(|r| r.ok()) {
            map.entry(row.0).or_default().push(row.1);
        }
        Ok(map)
    }

    fn attach_issues_batch(&self, conn: &Connection, verifications: &mut [Verification]) -> Result<()> {
        let ids: Vec<&str> = verifications.iter().map(|v| v.id.as_str()).collect();
        let mut map = self.load_issues_batch(conn, &ids)?;
        for v in verifications.iter_mut() {
            v.issues = map.remove(&v.id).unwrap_or_default();
        }
        Ok(())
    }

    fn save_issues(&self, conn: &Connection, verification: &Verification) -> Result<()> {
        // Delete existing issues first
        conn.execute(
            "DELETE FROM verification_issues WHERE verification_id = ?1",
            params![verification.id],
        )?;

        // Insert new issues
        let mut stmt = conn.prepare_cached(
            "INSERT INTO verification_issues
             (verification_id, file, line, severity, category, code, problem, suggestion)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;

        for issue in &verification.issues {
            stmt.execute(params![
                verification.id,
                issue.file,
                issue.line.map(|v| v as i32),
                issue.severity.to_string(),
                issue.category,
                issue.code,
                issue.problem,
                issue.suggestion,
            ])?;
        }

        Ok(())
    }
}

/// Add a verification record using an existing connection (for cross-store transactions).
///
/// Caller is responsible for managing the transaction. Does not save issues -
/// call `save_verification_issues_with_conn` separately.
pub fn add_verification_with_conn(conn: &Connection, verification: &Verification) -> Result<()> {
    let files_reviewed_json =
        serde_json::to_string(&verification.files_reviewed).unwrap_or_else(|_| "[]".to_string());

    conn.execute(
        "INSERT INTO verifications
         (id, task_id, agent_id, verification_type, status, confidence, summary, files_reviewed, duration_ms, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            verification.id,
            verification.task_id,
            verification.agent_id,
            verification.verification_type.to_string(),
            verification.status.to_string(),
            verification.confidence,
            verification.summary,
            files_reviewed_json,
            verification.duration_ms.map(|v| v as i64),
            verification.created_at.to_rfc3339(),
        ],
    )?;

    // Save issues inline
    save_verification_issues_with_conn(conn, verification)?;

    Ok(())
}

/// Save verification issues using an existing connection (for cross-store transactions).
pub fn save_verification_issues_with_conn(
    conn: &Connection,
    verification: &Verification,
) -> Result<()> {
    conn.execute(
        "DELETE FROM verification_issues WHERE verification_id = ?1",
        params![verification.id],
    )?;

    let mut stmt = conn.prepare_cached(
        "INSERT INTO verification_issues
         (verification_id, file, line, severity, category, code, problem, suggestion)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;

    for issue in &verification.issues {
        stmt.execute(params![
            verification.id,
            issue.file,
            issue.line.map(|v| v as i32),
            issue.severity.to_string(),
            issue.category,
            issue.code,
            issue.problem,
            issue.suggestion,
        ])?;
    }

    Ok(())
}

impl VerificationStore for SqliteVerificationStore {
    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_err)?;
        conn.execute_batch(VERIFICATION_SCHEMA)?;
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        use std::time::{SystemTime, UNIX_EPOCH};

        // The pre-cas-3bd4 implementation used `timestamp_millis & 0xffff`
        // (last 4 hex chars), which collides for any two calls landing
        // in the same millisecond — exactly what happens when a task
        // racks up a dispatch row and a skip row back-to-back during a
        // single close path. The collision triggers
        // `UNIQUE constraint failed: verifications.id` and silently
        // drops the newer row.
        //
        // Mix nanoseconds with a per-process random seed so rapid
        // successive calls produce distinct ids even inside the same
        // millisecond.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let rand: u32 = rand::random();
        // 8 hex chars from nanos + 4 from randomness = 48 bits of
        // collision surface, plenty for in-process use.
        Ok(format!("ver-{:08x}{:04x}", (nanos as u64) & 0xffff_ffff, rand & 0xffff))
    }

    fn add(&self, verification: &Verification) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let files_reviewed_json = serde_json::to_string(&verification.files_reviewed)
            .unwrap_or_else(|_| "[]".to_string());

        conn.execute(
            "INSERT INTO verifications
             (id, task_id, agent_id, verification_type, status, confidence, summary, files_reviewed, duration_ms, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                verification.id,
                verification.task_id,
                verification.agent_id,
                verification.verification_type.to_string(),
                verification.status.to_string(),
                verification.confidence,
                verification.summary,
                files_reviewed_json,
                verification.duration_ms.map(|v| v as i64),
                verification.created_at.to_rfc3339(),
            ],
        )?;

        // Save issues
        self.save_issues(&conn, verification)?;

        Ok(())
    }

    fn get(&self, id: &str) -> Result<Verification> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let mut verification = conn
            .query_row(
                "SELECT id, task_id, agent_id, verification_type, status, confidence, summary,
                        files_reviewed, duration_ms, created_at
                 FROM verifications WHERE id = ?1",
                params![id],
                Self::parse_verification,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(id.to_string()),
                _ => StoreError::Database(e),
            })?;

        verification.issues = self.load_issues(&conn, id)?;

        Ok(verification)
    }

    fn update(&self, verification: &Verification) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let files_reviewed_json = serde_json::to_string(&verification.files_reviewed)
            .unwrap_or_else(|_| "[]".to_string());

        let rows = conn.execute(
            "UPDATE verifications SET
             task_id = ?2, agent_id = ?3, verification_type = ?4, status = ?5, confidence = ?6,
             summary = ?7, files_reviewed = ?8, duration_ms = ?9, created_at = ?10
             WHERE id = ?1",
            params![
                verification.id,
                verification.task_id,
                verification.agent_id,
                verification.verification_type.to_string(),
                verification.status.to_string(),
                verification.confidence,
                verification.summary,
                files_reviewed_json,
                verification.duration_ms.map(|v| v as i64),
                verification.created_at.to_rfc3339(),
            ],
        )?;

        if rows == 0 {
            return Err(StoreError::NotFound(verification.id.clone()));
        }

        // Update issues
        self.save_issues(&conn, verification)?;

        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_err)?;

        // Issues deleted via CASCADE
        let rows = conn.execute("DELETE FROM verifications WHERE id = ?1", params![id])?;

        if rows == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }

        Ok(())
    }

    fn get_for_task(&self, task_id: &str) -> Result<Vec<Verification>> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, task_id, agent_id, verification_type, status, confidence, summary,
                    files_reviewed, duration_ms, created_at
             FROM verifications WHERE task_id = ?1
             ORDER BY created_at DESC",
        )?;

        let mut verifications: Vec<Verification> = stmt
            .query_map(params![task_id], Self::parse_verification)?
            .filter_map(|r| r.ok())
            .collect();

        self.attach_issues_batch(&conn, &mut verifications)?;

        Ok(verifications)
    }

    fn get_latest_for_task(&self, task_id: &str) -> Result<Option<Verification>> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let verification = conn
            .query_row(
                "SELECT id, task_id, agent_id, verification_type, status, confidence, summary,
                        files_reviewed, duration_ms, created_at
                 FROM verifications WHERE task_id = ?1
                 ORDER BY created_at DESC LIMIT 1",
                params![task_id],
                Self::parse_verification,
            )
            .optional()
            .map_err(StoreError::Database)?;

        match verification {
            Some(mut v) => {
                v.issues = self.load_issues(&conn, &v.id)?;
                Ok(Some(v))
            }
            None => Ok(None),
        }
    }

    fn get_latest_for_task_by_type(
        &self,
        task_id: &str,
        verification_type: VerificationType,
    ) -> Result<Option<Verification>> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let verification = conn
            .query_row(
                "SELECT id, task_id, agent_id, verification_type, status, confidence, summary,
                        files_reviewed, duration_ms, created_at
                 FROM verifications WHERE task_id = ?1 AND verification_type = ?2
                 ORDER BY created_at DESC LIMIT 1",
                params![task_id, verification_type.to_string()],
                Self::parse_verification,
            )
            .optional()
            .map_err(StoreError::Database)?;

        match verification {
            Some(mut v) => {
                v.issues = self.load_issues(&conn, &v.id)?;
                Ok(Some(v))
            }
            None => Ok(None),
        }
    }

    fn list_recent(&self, limit: usize) -> Result<Vec<Verification>> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, task_id, agent_id, verification_type, status, confidence, summary,
                    files_reviewed, duration_ms, created_at
             FROM verifications ORDER BY created_at DESC LIMIT ?1",
        )?;

        let mut verifications: Vec<Verification> = stmt
            .query_map(params![limit as i32], Self::parse_verification)?
            .filter_map(|r| r.ok())
            .collect();

        self.attach_issues_batch(&conn, &mut verifications)?;

        Ok(verifications)
    }

    fn list_by_status(&self, status: VerificationStatus) -> Result<Vec<Verification>> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, task_id, agent_id, verification_type, status, confidence, summary,
                    files_reviewed, duration_ms, created_at
             FROM verifications WHERE status = ?1
             ORDER BY created_at DESC",
        )?;

        let mut verifications: Vec<Verification> = stmt
            .query_map(params![status.to_string()], Self::parse_verification)?
            .filter_map(|r| r.ok())
            .collect();

        self.attach_issues_batch(&conn, &mut verifications)?;

        Ok(verifications)
    }

    fn prune(&self, older_than_days: i64) -> Result<usize> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let cutoff = (Utc::now() - chrono::Duration::days(older_than_days)).to_rfc3339();

        // Issues are deleted via CASCADE
        let rows = conn.execute(
            "DELETE FROM verifications WHERE created_at < ?",
            params![cutoff],
        )?;

        Ok(rows)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::verification_store::*;
    use tempfile::TempDir;

    fn create_test_store() -> (SqliteVerificationStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = SqliteVerificationStore::open(dir.path()).unwrap();
        (store, dir)
    }

    #[test]
    fn test_add_and_get_verification() {
        let (store, _dir) = create_test_store();

        let verification = Verification::approved(
            "ver-test".to_string(),
            "cas-1234".to_string(),
            "All checks passed".to_string(),
        );

        store.add(&verification).unwrap();

        let retrieved = store.get("ver-test").unwrap();
        assert_eq!(retrieved.id, "ver-test");
        assert_eq!(retrieved.task_id, "cas-1234");
        assert!(retrieved.is_approved());
        assert_eq!(retrieved.summary, "All checks passed");
    }

    #[test]
    fn test_verification_with_issues() {
        let (store, _dir) = create_test_store();

        let issues = vec![
            VerificationIssue::blocking(
                "src/main.rs".to_string(),
                Some(42),
                "todo_comment".to_string(),
                "// TODO: implement".to_string(),
                "TODO comment found".to_string(),
                Some("Implement the function".to_string()),
            ),
            VerificationIssue::warning(
                "src/lib.rs".to_string(),
                "hardcoded_value".to_string(),
                "Magic number detected".to_string(),
            ),
        ];

        let verification = Verification::rejected(
            "ver-test".to_string(),
            "cas-1234".to_string(),
            "Found incomplete work".to_string(),
            issues,
        );

        store.add(&verification).unwrap();

        let retrieved = store.get("ver-test").unwrap();
        assert!(retrieved.is_rejected());
        assert_eq!(retrieved.issues.len(), 2);
        assert_eq!(retrieved.blocking_count(), 1);
        assert_eq!(retrieved.warning_count(), 1);

        let first_issue = &retrieved.issues[0];
        assert_eq!(first_issue.file, "src/main.rs");
        assert_eq!(first_issue.line, Some(42));
        assert!(first_issue.is_blocking());
    }

    #[test]
    fn test_update_verification() {
        let (store, _dir) = create_test_store();

        let mut verification = Verification::new("ver-test".to_string(), "cas-1234".to_string());
        store.add(&verification).unwrap();

        // Update with new status and issues
        verification.status = VerificationStatus::Rejected;
        verification.summary = "Found issues".to_string();
        verification.issues.push(VerificationIssue::new(
            "src/api.rs".to_string(),
            "stub".to_string(),
            "Stub implementation".to_string(),
        ));

        store.update(&verification).unwrap();

        let retrieved = store.get("ver-test").unwrap();
        assert!(retrieved.is_rejected());
        assert_eq!(retrieved.issues.len(), 1);
    }

    #[test]
    fn test_get_for_task() {
        let (store, _dir) = create_test_store();

        // Add multiple verifications for same task
        for i in 0..3 {
            let verification = Verification::approved(
                format!("ver-{i}"),
                "cas-1234".to_string(),
                format!("Attempt {i}"),
            );
            store.add(&verification).unwrap();
        }

        // Add one for different task
        let other = Verification::approved(
            "ver-other".to_string(),
            "cas-5678".to_string(),
            "Other task".to_string(),
        );
        store.add(&other).unwrap();

        let task_verifications = store.get_for_task("cas-1234").unwrap();
        assert_eq!(task_verifications.len(), 3);
    }

    #[test]
    fn test_get_latest_for_task() {
        let (store, _dir) = create_test_store();

        // No verifications initially
        let latest = store.get_latest_for_task("cas-1234").unwrap();
        assert!(latest.is_none());

        // Add verifications
        let v1 = Verification::rejected(
            "ver-1".to_string(),
            "cas-1234".to_string(),
            "First attempt".to_string(),
            vec![],
        );
        store.add(&v1).unwrap();

        let v2 = Verification::approved(
            "ver-2".to_string(),
            "cas-1234".to_string(),
            "Second attempt".to_string(),
        );
        store.add(&v2).unwrap();

        let latest = store.get_latest_for_task("cas-1234").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().id, "ver-2");
    }

    #[test]
    fn test_get_latest_for_task_by_type() {
        let (store, _dir) = create_test_store();

        // No verifications initially
        let latest = store
            .get_latest_for_task_by_type("cas-1234", VerificationType::Task)
            .unwrap();
        assert!(latest.is_none());

        let latest = store
            .get_latest_for_task_by_type("cas-1234", VerificationType::Epic)
            .unwrap();
        assert!(latest.is_none());

        // Add a task-type verification
        let mut v1 = Verification::approved(
            "ver-task-1".to_string(),
            "cas-1234".to_string(),
            "Task verification".to_string(),
        );
        v1.verification_type = VerificationType::Task;
        store.add(&v1).unwrap();

        // Add an epic-type verification
        let mut v2 = Verification::rejected(
            "ver-epic-1".to_string(),
            "cas-1234".to_string(),
            "Epic verification".to_string(),
            vec![],
        );
        v2.verification_type = VerificationType::Epic;
        store.add(&v2).unwrap();

        // Add another task-type verification (newer)
        let mut v3 = Verification::approved(
            "ver-task-2".to_string(),
            "cas-1234".to_string(),
            "Second task verification".to_string(),
        );
        v3.verification_type = VerificationType::Task;
        store.add(&v3).unwrap();

        // Get latest task verification - should be v3
        let latest_task = store
            .get_latest_for_task_by_type("cas-1234", VerificationType::Task)
            .unwrap();
        assert!(latest_task.is_some());
        assert_eq!(latest_task.unwrap().id, "ver-task-2");

        // Get latest epic verification - should be v2
        let latest_epic = store
            .get_latest_for_task_by_type("cas-1234", VerificationType::Epic)
            .unwrap();
        assert!(latest_epic.is_some());
        assert_eq!(latest_epic.unwrap().id, "ver-epic-1");

        // Different task has no verifications
        let latest_other = store
            .get_latest_for_task_by_type("cas-5678", VerificationType::Task)
            .unwrap();
        assert!(latest_other.is_none());
    }

    #[test]
    fn test_list_by_status() {
        let (store, _dir) = create_test_store();

        let approved = Verification::approved(
            "ver-approved".to_string(),
            "cas-1".to_string(),
            "Good".to_string(),
        );
        store.add(&approved).unwrap();

        let rejected = Verification::rejected(
            "ver-rejected".to_string(),
            "cas-2".to_string(),
            "Bad".to_string(),
            vec![],
        );
        store.add(&rejected).unwrap();

        let approved_list = store.list_by_status(VerificationStatus::Approved).unwrap();
        assert_eq!(approved_list.len(), 1);
        assert_eq!(approved_list[0].id, "ver-approved");

        let rejected_list = store.list_by_status(VerificationStatus::Rejected).unwrap();
        assert_eq!(rejected_list.len(), 1);
        assert_eq!(rejected_list[0].id, "ver-rejected");
    }

    #[test]
    fn test_delete_verification() {
        let (store, _dir) = create_test_store();

        let verification = Verification::approved(
            "ver-test".to_string(),
            "cas-1234".to_string(),
            "Good".to_string(),
        );
        store.add(&verification).unwrap();

        store.delete("ver-test").unwrap();

        let result = store.get("ver-test");
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_id() {
        let (store, _dir) = create_test_store();

        let id = store.generate_id().unwrap();
        assert!(id.starts_with("ver-"));
        assert!(id.len() > 4);
    }

    #[test]
    fn test_list_recent() {
        let (store, _dir) = create_test_store();

        for i in 0..5 {
            let verification =
                Verification::approved(format!("ver-{i}"), format!("cas-{i}"), format!("Task {i}"));
            store.add(&verification).unwrap();
        }

        let recent = store.list_recent(3).unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[test]
    fn test_files_reviewed_persistence() {
        let (store, _dir) = create_test_store();

        let mut verification = Verification::approved(
            "ver-test".to_string(),
            "cas-1234".to_string(),
            "Done".to_string(),
        );
        verification.add_file_reviewed("src/main.rs".to_string());
        verification.add_file_reviewed("src/lib.rs".to_string());
        verification.add_file_reviewed("tests/test.rs".to_string());

        store.add(&verification).unwrap();

        let retrieved = store.get("ver-test").unwrap();
        assert_eq!(retrieved.files_reviewed.len(), 3);
        assert!(
            retrieved
                .files_reviewed
                .contains(&"src/main.rs".to_string())
        );
    }

    #[test]
    fn test_verification_with_confidence() {
        let (store, _dir) = create_test_store();

        let mut verification = Verification::approved(
            "ver-test".to_string(),
            "cas-1234".to_string(),
            "High confidence".to_string(),
        );
        verification.set_confidence(0.95);

        store.add(&verification).unwrap();

        let retrieved = store.get("ver-test").unwrap();
        assert_eq!(retrieved.confidence, Some(0.95));
    }
}
