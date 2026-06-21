//! DDL for the SQLite store. See `docs/SCHEMA.md` for the rationale.
//!
//! Migrations are forward-only and keyed by `schema_version`. To evolve the
//! schema, append a new `&str` to [`MIGRATIONS`]; never edit an existing one.

/// Ordered migrations. Index + 1 == the schema version it brings the DB to.
pub const MIGRATIONS: &[&str] = &[
    // v1 — initial schema
    r#"
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );

    CREATE TABLE profiles (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        name          TEXT NOT NULL UNIQUE,
        kind          TEXT NOT NULL,
        operation     TEXT NOT NULL,
        source        TEXT NOT NULL,
        destination   TEXT NOT NULL,
        options_json  TEXT NOT NULL,
        secret_ref    TEXT,
        created_at    INTEGER NOT NULL,
        updated_at    INTEGER NOT NULL
    );

    CREATE TABLE assistant_templates (
        id                    INTEGER PRIMARY KEY AUTOINCREMENT,
        scenario              TEXT NOT NULL UNIQUE,
        title                 TEXT NOT NULL,
        description           TEXT NOT NULL,
        risk_level            TEXT NOT NULL,
        default_options_json  TEXT NOT NULL,
        builtin               INTEGER NOT NULL DEFAULT 1
    );

    CREATE TABLE jobs (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        name          TEXT NOT NULL,
        profile_id    INTEGER REFERENCES profiles(id) ON DELETE SET NULL,
        kind          TEXT NOT NULL,
        operation     TEXT NOT NULL,
        source        TEXT NOT NULL,
        destination   TEXT NOT NULL,
        options_json  TEXT NOT NULL,
        schedule_cron TEXT,
        created_at    INTEGER NOT NULL
    );

    CREATE TABLE job_runs (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        job_id            INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
        status            TEXT NOT NULL,
        dry_run           INTEGER NOT NULL DEFAULT 0,
        argv_preview      TEXT NOT NULL,
        exit_code         INTEGER,
        bytes_transferred INTEGER NOT NULL DEFAULT 0,
        files_done        INTEGER NOT NULL DEFAULT 0,
        avg_speed_bps     INTEGER,
        started_at        INTEGER,
        ended_at          INTEGER,
        error_summary     TEXT
    );

    CREATE TABLE run_logs (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        run_id            INTEGER NOT NULL REFERENCES job_runs(id) ON DELETE CASCADE,
        log_path          TEXT NOT NULL,
        level_counts_json TEXT
    );

    CREATE INDEX idx_job_runs_job ON job_runs(job_id);
    CREATE INDEX idx_run_logs_run ON run_logs(run_id);
    "#,
];
