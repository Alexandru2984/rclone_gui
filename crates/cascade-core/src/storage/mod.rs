//! SQLite persistence (bundled SQLite via `rusqlite`, no system dependency).
//!
//! Holds job history, profiles, settings, and log metadata. **No secrets** are
//! stored here — see the threat model.

pub mod repo;
pub mod schema;

pub use repo::{ProfileRecord, RunRecord};

use rusqlite::Connection;

use crate::error::{CoreError, Result};
use crate::security::sanitize;

/// An open database handle.
pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    /// Open (or create) the database at `path` and run pending migrations.
    pub fn open(path: &std::path::Path) -> Result<Self> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => {
                return Err(CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "database path must be a real file, not a symlink",
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(CoreError::Io(error)),
        }
        let conn = Connection::open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
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
        conn.pragma_update(None, "secure_delete", "ON")?;
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
        if sanitize::contains_secret(value) {
            return Err(CoreError::InvalidCommand(
                "refusing to persist a setting containing a credential".into(),
            ));
        }
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
    fn reopening_a_file_db_preserves_data_and_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cascade.db");

        // First open: create + migrate, write a profile and a queue item.
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(
                store.current_version().unwrap(),
                schema::MIGRATIONS.len() as i64
            );
            store
                .insert_job_raw("j", "rsync", "copy", "/a", "/b", "{}")
                .unwrap();
            store.set_setting("k", "v").unwrap();
        }
        // Reopen: migrations are a no-op, and data survives.
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(
                store.current_version().unwrap(),
                schema::MIGRATIONS.len() as i64
            );
            assert_eq!(store.get_setting("k").unwrap().as_deref(), Some("v"));
            assert_eq!(store.recent_runs(10).unwrap().len(), 0); // job but no run yet
        }
    }

    #[test]
    fn missing_setting_is_none() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.get_setting("does-not-exist").unwrap(), None);
    }

    #[test]
    fn setting_can_hold_empty_and_unicode_values() {
        let store = Store::open_in_memory().unwrap();
        store.set_setting("empty", "").unwrap();
        store.set_setting("uni", "café 🚀").unwrap();
        assert_eq!(store.get_setting("empty").unwrap().as_deref(), Some(""));
        assert_eq!(
            store.get_setting("uni").unwrap().as_deref(),
            Some("café 🚀")
        );
    }

    #[test]
    fn expected_tables_exist() {
        let store = Store::open_in_memory().unwrap();
        let count: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN
                 ('settings','profiles','assistant_templates','jobs','job_runs','run_logs','queue_items')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 7);
    }

    #[cfg(unix)]
    #[test]
    fn file_database_is_private_and_symlinks_are_refused() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cascade.db");
        drop(Store::open(&path).unwrap());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        let link = dir.path().join("linked.db");
        symlink(&path, &link).unwrap();
        assert!(Store::open(&link).is_err());
    }
}
