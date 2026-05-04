//! Migration: Add `worker_spec` column to `spawn_queue`.
//!
//! Adds a nullable TEXT column that stores a JSON-serialised `WorkerSpec`
//! produced by `cas-factory`'s cascade resolver.  Rows enqueued before this
//! migration have `NULL` for `worker_spec`, which the daemon interprets as
//! "use the session default" — preserving backwards compatibility.
//!
//! Added in cas-2992 (T3 of EPIC cas-b3db): CLI + MCP per-worker spec overrides.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 201,
    name: "spawn_queue_add_worker_spec",
    subsystem: Subsystem::Agents,
    description: "Add worker_spec TEXT column to spawn_queue for per-worker CLI/model/effort overrides (cas-2992)",
    up: &[
        "ALTER TABLE spawn_queue ADD COLUMN worker_spec TEXT",
    ],
    detect: Some(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('spawn_queue') WHERE name = 'worker_spec') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn spawn_queue_columns(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('spawn_queue') ORDER BY cid")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn migration_adds_worker_spec_column() {
        let conn = Connection::open_in_memory().unwrap();

        // Simulate existing spawn_queue without worker_spec (pre-migration schema).
        conn.execute_batch(
            "CREATE TABLE spawn_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                action TEXT NOT NULL,
                count INTEGER,
                worker_names TEXT,
                force INTEGER NOT NULL DEFAULT 0,
                isolate INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                processed_at TEXT
            );",
        )
        .unwrap();

        // Verify detect returns 0 (not yet applied).
        let result: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(result, 0, "detect should return 0 on pre-migration schema");

        // Apply migration.
        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        let cols = spawn_queue_columns(&conn);
        assert!(
            cols.contains(&"worker_spec".to_string()),
            "worker_spec column should exist after migration"
        );

        // Verify detect returns 1 (already applied).
        let result: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(result, 1, "detect should return 1 after migration");
    }

    #[test]
    fn idempotent_detect_on_fresh_schema() {
        let conn = Connection::open_in_memory().unwrap();

        // Schema already has worker_spec (fresh DB after migration).
        conn.execute_batch(
            "CREATE TABLE spawn_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                action TEXT NOT NULL,
                count INTEGER,
                worker_names TEXT,
                force INTEGER NOT NULL DEFAULT 0,
                isolate INTEGER NOT NULL DEFAULT 0,
                worker_spec TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                processed_at TEXT
            );",
        )
        .unwrap();

        let result: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(result, 1, "detect must return 1 when column already exists");
    }

    #[test]
    fn spec_column_accepts_json_and_null() {
        let conn = Connection::open_in_memory().unwrap();

        conn.execute_batch(
            "CREATE TABLE spawn_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                action TEXT NOT NULL,
                count INTEGER,
                worker_names TEXT,
                force INTEGER NOT NULL DEFAULT 0,
                isolate INTEGER NOT NULL DEFAULT 0,
                worker_spec TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                processed_at TEXT
            );",
        )
        .unwrap();

        // NULL (legacy / no override)
        conn.execute(
            "INSERT INTO spawn_queue (action, count, worker_spec) VALUES ('spawn', 1, NULL)",
            [],
        )
        .unwrap();

        // JSON spec
        let spec_json = r#"{"name":null,"cli":"codex","model":null,"effort":"high"}"#;
        conn.execute(
            "INSERT INTO spawn_queue (action, count, worker_spec) VALUES ('spawn', 1, ?)",
            rusqlite::params![spec_json],
        )
        .unwrap();

        // Read back and verify
        let rows: Vec<Option<String>> = {
            let mut stmt = conn
                .prepare("SELECT worker_spec FROM spawn_queue ORDER BY id")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, Option<String>>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        assert_eq!(rows.len(), 2);
        assert!(rows[0].is_none(), "first row should have NULL worker_spec");
        let json = rows[1].as_deref().unwrap();
        assert!(json.contains("codex"), "second row spec should contain 'codex': {json}");
    }
}
