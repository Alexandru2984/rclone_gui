//! Job/run persistence on top of [`Store`].
//!
//! These are the read/write helpers the GUI uses to record what it ran and to
//! populate the History screen. No secrets pass through here — only the
//! sanitized command preview and run metadata.

use rusqlite::params;

use crate::error::Result;

use super::Store;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// One row for the History list (a run joined with its job).
#[derive(Debug, Clone)]
pub struct RunRecord {
    pub run_id: i64,
    pub job_name: String,
    pub kind: String,
    pub operation: String,
    pub status: String,
    pub dry_run: bool,
    pub argv_preview: String,
    pub exit_code: Option<i64>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

impl Store {
    /// Insert a configured job, returning its id.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_job(
        &self,
        name: &str,
        kind: &str,
        operation: &str,
        source: &str,
        destination: &str,
        options_json: &str,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO jobs (name, kind, operation, source, destination, options_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![name, kind, operation, source, destination, options_json, now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Record the start of a run (status = running), returning its id.
    pub fn start_run(&self, job_id: i64, dry_run: bool, argv_preview: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO job_runs (job_id, status, dry_run, argv_preview, started_at)
             VALUES (?1, 'running', ?2, ?3, ?4)",
            params![job_id, dry_run as i64, argv_preview, now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Mark a run finished with a terminal status and optional exit code / error.
    pub fn finish_run(
        &self,
        run_id: i64,
        status: &str,
        exit_code: Option<i32>,
        error_summary: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE job_runs
             SET status = ?1, exit_code = ?2, ended_at = ?3, error_summary = ?4
             WHERE id = ?5",
            params![status, exit_code, now(), error_summary, run_id],
        )?;
        Ok(())
    }

    /// Most recent runs, newest first, for the History screen.
    pub fn recent_runs(&self, limit: i64) -> Result<Vec<RunRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, j.name, j.kind, j.operation, r.status, r.dry_run,
                    r.argv_preview, r.exit_code, r.started_at, r.ended_at
             FROM job_runs r JOIN jobs j ON j.id = r.job_id
             ORDER BY r.id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |row| {
            Ok(RunRecord {
                run_id: row.get(0)?,
                job_name: row.get(1)?,
                kind: row.get(2)?,
                operation: row.get(3)?,
                status: row.get(4)?,
                dry_run: row.get::<_, i64>(5)? != 0,
                argv_preview: row.get(6)?,
                exit_code: row.get(7)?,
                started_at: row.get(8)?,
                ended_at: row.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_run_lifecycle_and_history() {
        let store = Store::open_in_memory().unwrap();

        let job_id = store
            .insert_job("nightly", "rsync", "sync", "/src/", "/dst/", r#"{"dry_run":false}"#)
            .unwrap();
        let run_id = store.start_run(job_id, true, "rsync -a -n /src/ /dst/").unwrap();

        // Mid-run it shows as running.
        let runs = store.recent_runs(10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "running");
        assert!(runs[0].dry_run);
        assert_eq!(runs[0].job_name, "nightly");

        store.finish_run(run_id, "completed", Some(0), None).unwrap();
        let runs = store.recent_runs(10).unwrap();
        assert_eq!(runs[0].status, "completed");
        assert_eq!(runs[0].exit_code, Some(0));
        assert!(runs[0].ended_at.is_some());
    }

    #[test]
    fn history_is_newest_first() {
        let store = Store::open_in_memory().unwrap();
        let j = store.insert_job("j", "rsync", "copy", "/a", "/b", "{}").unwrap();
        let r1 = store.start_run(j, false, "cmd1").unwrap();
        let r2 = store.start_run(j, false, "cmd2").unwrap();
        let runs = store.recent_runs(10).unwrap();
        assert_eq!(runs[0].run_id, r2);
        assert_eq!(runs[1].run_id, r1);
    }
}
