//! SQLite persistence (bundled SQLite via `rusqlite`, no system dependency).
//!
//! Holds job history, profiles, settings, and log metadata. **No secrets** are
//! stored here — see the threat model.

pub mod repo;
pub mod schema;

pub use repo::RunRecord;

use rusqlite::Connection;

use crate::error::Result;

/// An open database handle.
pub struct Store {
    pub conn: Connection,
}

impl Store {
    /// Open (or create) the database at `path` and run pending migrations.
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Open an in-memory database (used by tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);",
        )?;

        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn current_version(&self) -> Result<i64> {
        let v: Option<i64> = self
            .conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
                r.get(0)
            })
            .ok();
        Ok(v.unwrap_or(0))
    }

    /// Apply any migrations newer than the stored version, transactionally.
    fn migrate(&mut self) -> Result<()> {
        let current = self.current_version()?;
        let target = schema::MIGRATIONS.len() as i64;
        if current >= target {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        for (i, ddl) in schema::MIGRATIONS.iter().enumerate() {
            let version = (i + 1) as i64;
            if version > current {
                tx.execute_batch(ddl)?;
            }
        }
        tx.execute("DELETE FROM schema_version", [])?;
        tx.execute("INSERT INTO schema_version (version) VALUES (?1)", [target])?;
        tx.commit()?;
        Ok(())
    }

    /// Convenience: read a setting value.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let v = self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .ok();
        Ok(v)
    }

    /// Convenience: write a setting value (upsert).
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_to_latest_version() {
        let store = Store::open_in_memory().unwrap();
        let v = store.current_version().unwrap();
        assert_eq!(v, schema::MIGRATIONS.len() as i64);
    }

    #[test]
    fn migration_is_idempotent() {
        let mut store = Store::open_in_memory().unwrap();
        store.migrate().unwrap(); // running again must be a no-op
        assert_eq!(
            store.current_version().unwrap(),
            schema::MIGRATIONS.len() as i64
        );
    }

    #[test]
    fn settings_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.get_setting("theme").unwrap(), None);
        store.set_setting("theme", "dark").unwrap();
        store.set_setting("theme", "light").unwrap(); // upsert
        assert_eq!(store.get_setting("theme").unwrap(), Some("light".into()));
    }

    #[test]
    fn expected_tables_exist() {
        let store = Store::open_in_memory().unwrap();
        let count: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN
                 ('settings','profiles','assistant_templates','jobs','job_runs','run_logs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 6);
    }
}
