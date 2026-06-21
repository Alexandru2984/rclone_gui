//! Job/run persistence on top of [`Store`].
//!
//! These are the read/write helpers the GUI uses to record what it ran and to
//! populate the History screen. No secrets pass through here — only the
//! sanitized command preview and run metadata.

use rusqlite::params;

use crate::error::Result;
use crate::job::JobSpec;

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

/// A saved profile and its reconstructed spec.
#[derive(Debug, Clone)]
pub struct ProfileRecord {
    pub id: i64,
    pub name: String,
    pub spec: JobSpec,
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

    /// Record the on-disk log for a finished run.
    pub fn insert_run_log(
        &self,
        run_id: i64,
        log_path: &str,
        level_counts_json: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO run_logs (run_id, log_path, level_counts_json) VALUES (?1, ?2, ?3)",
            params![run_id, log_path, level_counts_json],
        )?;
        Ok(())
    }

    /// Save (or update by name) a reusable profile from a [`JobSpec`].
    pub fn save_profile(&self, spec: &JobSpec) -> Result<i64> {
        let options_json = serde_json::to_string(spec)?;
        let kind = match spec.tool {
            crate::Tool::Rclone => "rclone",
            crate::Tool::Rsync => "rsync",
        };
        let operation = match spec.op {
            crate::job::OpKind::Copy => "copy",
            crate::job::OpKind::Sync => "sync",
            crate::job::OpKind::Move => "move",
        };
        let now = now();
        self.conn.execute(
            "INSERT INTO profiles
                (name, kind, operation, source, destination, options_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(name) DO UPDATE SET
                kind = excluded.kind, operation = excluded.operation,
                source = excluded.source, destination = excluded.destination,
                options_json = excluded.options_json, updated_at = excluded.updated_at",
            params![
                spec.name,
                kind,
                operation,
                spec.source,
                spec.destination,
                options_json,
                now
            ],
        )?;
        // last_insert_rowid is 0 on a pure UPDATE; resolve the id by name.
        let id: i64 = self.conn.query_row(
            "SELECT id FROM profiles WHERE name = ?1",
            [&spec.name],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    /// All saved profiles, with their reconstructed specs, newest first.
    pub fn list_profiles(&self) -> Result<Vec<ProfileRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, options_json FROM profiles ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let name: String = row.get(1)?;
            let options_json: String = row.get(2)?;
            Ok((id, name, options_json))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, options_json) = row?;
            if let Ok(spec) = serde_json::from_str::<JobSpec>(&options_json) {
                out.push(ProfileRecord { id, name, spec });
            }
        }
        Ok(out)
    }

    /// Delete a profile by id.
    pub fn delete_profile(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM profiles WHERE id = ?1", [id])?;
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
            .insert_job(
                "nightly",
                "rsync",
                "sync",
                "/src/",
                "/dst/",
                r#"{"dry_run":false}"#,
            )
            .unwrap();
        let run_id = store
            .start_run(job_id, true, "rsync -a -n /src/ /dst/")
            .unwrap();

        // Mid-run it shows as running.
        let runs = store.recent_runs(10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "running");
        assert!(runs[0].dry_run);
        assert_eq!(runs[0].job_name, "nightly");

        store
            .finish_run(run_id, "completed", Some(0), None)
            .unwrap();
        let runs = store.recent_runs(10).unwrap();
        assert_eq!(runs[0].status, "completed");
        assert_eq!(runs[0].exit_code, Some(0));
        assert!(runs[0].ended_at.is_some());
    }

    #[test]
    fn profiles_save_list_update_delete() {
        use crate::job::OpKind;
        use crate::Tool;

        let store = Store::open_in_memory().unwrap();
        let mut spec = JobSpec {
            name: "photos".into(),
            tool: Tool::Rsync,
            op: OpKind::Sync,
            source: "/src/".into(),
            destination: "/dst/".into(),
            dry_run: false,
            delete: false,
        };
        let id1 = store.save_profile(&spec).unwrap();

        let profiles = store.list_profiles().unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "photos");
        assert_eq!(profiles[0].spec.op, OpKind::Sync);

        // Saving under the same name updates in place (no duplicate).
        spec.op = OpKind::Copy;
        let id2 = store.save_profile(&spec).unwrap();
        assert_eq!(id1, id2);
        let profiles = store.list_profiles().unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].spec.op, OpKind::Copy);

        store.delete_profile(id1).unwrap();
        assert!(store.list_profiles().unwrap().is_empty());
    }

    #[test]
    fn run_log_can_be_recorded() {
        let store = Store::open_in_memory().unwrap();
        let job = store
            .insert_job("j", "rsync", "copy", "/a", "/b", "{}")
            .unwrap();
        let run = store.start_run(job, false, "cmd").unwrap();
        store
            .insert_run_log(
                run,
                "/tmp/run-1.log",
                r#"{"errors":0,"warnings":1,"info":3}"#,
            )
            .unwrap();
        let count: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM run_logs WHERE run_id = ?1",
                [run],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn history_is_newest_first() {
        let store = Store::open_in_memory().unwrap();
        let j = store
            .insert_job("j", "rsync", "copy", "/a", "/b", "{}")
            .unwrap();
        let r1 = store.start_run(j, false, "cmd1").unwrap();
        let r2 = store.start_run(j, false, "cmd2").unwrap();
        let runs = store.recent_runs(10).unwrap();
        assert_eq!(runs[0].run_id, r2);
        assert_eq!(runs[1].run_id, r1);
    }
}
