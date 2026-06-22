# D. SQLite data model

`rusqlite` with bundled SQLite, WAL mode. Forward-only migrations keyed by `schema_version`.

**Cascade never stores secrets — anywhere.** Credentials (cloud tokens, SSH/SFTP
passwords) are owned and encrypted by rclone's own config; we only invoke rclone and
reference remotes as `remote:path`. To enforce this, `save_profile` **refuses** any spec
whose paths or flags embed a credential, directing the user to `rclone config` instead.
There is deliberately no secret/keyring column.

## Tables

### `schema_version`
| column | type | notes |
|---|---|---|
| version | INTEGER PRIMARY KEY | current migration level |

### `settings`
Key/value app settings (theme, max parallel jobs, default dry-run, rcd enabled…).
| column | type | notes |
|---|---|---|
| key | TEXT PRIMARY KEY | |
| value | TEXT | JSON-encoded |

### `profiles`
Reusable, named operation templates (the app's own profiles — not rclone's native config).
| column | type | notes |
|---|---|---|
| id | INTEGER PK AUTOINCREMENT | |
| name | TEXT UNIQUE NOT NULL | |
| kind | TEXT NOT NULL | `rclone` \| `rsync` |
| operation | TEXT NOT NULL | copy/sync/move/check/backup/mirror… |
| source | TEXT NOT NULL | |
| destination | TEXT NOT NULL | |
| options_json | TEXT NOT NULL | serialized options struct (secret-free; save is refused otherwise) |
| created_at | INTEGER NOT NULL | unix epoch |
| updated_at | INTEGER NOT NULL | |

### `assistant_templates`
Backup Assistant scenarios (seeded built-ins + user-customized).
| column | type | notes |
|---|---|---|
| id | INTEGER PK AUTOINCREMENT | |
| scenario | TEXT UNIQUE NOT NULL | `backup_photos`, `backup_to_gdrive`, … |
| title | TEXT NOT NULL | |
| description | TEXT NOT NULL | |
| risk_level | TEXT NOT NULL | `safe` \| `caution` \| `destructive` |
| default_options_json | TEXT NOT NULL | |
| builtin | INTEGER NOT NULL | 0/1 |

### `jobs`
A configured job (may be run many times). FK → `profiles` (nullable; ad-hoc jobs allowed).
| column | type | notes |
|---|---|---|
| id | INTEGER PK AUTOINCREMENT | |
| name | TEXT NOT NULL | |
| profile_id | INTEGER | FK profiles(id) ON DELETE SET NULL |
| kind | TEXT NOT NULL | rclone/rsync |
| operation | TEXT NOT NULL | |
| source | TEXT NOT NULL | |
| destination | TEXT NOT NULL | |
| options_json | TEXT NOT NULL | |
| schedule_cron | TEXT | nullable, for recurring jobs |
| created_at | INTEGER NOT NULL | |

### `job_runs`
One execution of a job — the heart of History & live status.
| column | type | notes |
|---|---|---|
| id | INTEGER PK AUTOINCREMENT | |
| job_id | INTEGER NOT NULL | FK jobs(id) ON DELETE CASCADE |
| status | TEXT NOT NULL | pending/running/paused/completed/failed/cancelled |
| dry_run | INTEGER NOT NULL | 0/1 |
| argv_preview | TEXT NOT NULL | sanitized generated command |
| exit_code | INTEGER | nullable |
| bytes_transferred | INTEGER NOT NULL DEFAULT 0 | |
| files_done | INTEGER NOT NULL DEFAULT 0 | |
| avg_speed_bps | INTEGER | nullable |
| started_at | INTEGER | nullable |
| ended_at | INTEGER | nullable |
| error_summary | TEXT | nullable, sanitized |

### `run_logs`
Log lines metadata + the path to the on-disk sanitized log file.
| column | type | notes |
|---|---|---|
| id | INTEGER PK AUTOINCREMENT | |
| run_id | INTEGER NOT NULL | FK job_runs(id) ON DELETE CASCADE |
| log_path | TEXT NOT NULL | sanitized log file on disk |
| level_counts_json | TEXT | {errors,warnings,info} tallies |

## Relationships
```
profiles 1───* jobs 1───* job_runs 1───1 run_logs
assistant_templates ──(seeds)──▶ profiles/jobs
settings, schema_version  (standalone)
```
Full log *text* lives on disk (rotated files under the data dir); SQLite stores metadata
and pointers, keeping the DB small and fast.
