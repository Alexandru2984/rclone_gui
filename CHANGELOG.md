# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security
- Refuse rclone RC below 1.73.5 to avoid CVE-2026-41176 and CVE-2026-41179.
- Reject custom flags that can negate dry-run/deletion guards, alter config or
  logging, inject operands, or execute remote commands; place endpoints after `--`.
- Reject credentials before argv construction or persistence; purge legacy
  credential-bearing database rows and fully redact multiline private keys.
- Canonicalize and revalidate paths at the execution boundary, including restored
  queue items, backup directories, symlink changes, roots, and overlaps.
- Bound process events, line/capture sizes, timeouts, GUI scrollback, and queue
  concurrency; terminate complete process groups on cancellation and teardown.
- Make recurring schedules dry-run by default and require explicit consent for
  live schedules; validate calendar input and install private units without
  following symlinks.
- Pin toolchains, GitHub Actions, release tools, Flatpak archives, and offline
  Cargo sources; isolate the release token and publish checksums plus provenance.
- Block inline remote credentials from `rclone config create` process arguments.
- Refresh and reduce the dependency graph, including the bundled SQLite stack.

## [0.1.1] — 2026-07-04

### Added
- **Two-way sync** via rclone `bisync`, with a `--resync` first-run toggle.
- **`--max-delete` safety net** — abort a run that would delete more than N files.
- **`--backup-dir`** — move replaced/deleted files aside instead of removing them,
  making a mirror/sync reversible.
- **Structured dry-run summary** — a one-line "N new · N updated · N to delete"
  digest parsed from rsync/rclone dry-run output.
- The **Jobs Queue is persisted** and resumed across restarts (pending jobs only;
  never credentials).
- **Desktop notification when a scheduled job fails** (systemd `OnFailure=`).
- **Re-run** a past job straight from its History / Job Details.

### Fixed
- **"Add to queue" now goes through the destructive-operation confirmation gate** —
  previously a mirror/move could be queued and run unattended without a prompt.
- **Source/destination overlap** (identical, or one inside the other) is detected,
  shown in the risk badge, and forces the confirmation gate.
- `tr("")` no longer renders the gettext catalog header; risk badges, completion
  notifications and other strings are now translated (catalog updated).
- The local RC daemon **fails loudly** instead of falling back to an all-zero
  password if the OS CSPRNG can't be read.
- Broader dangerous-flag detection (`--del`, `--remove-sent-files`,
  `--password-command`).
- New Job live-log scrollback is bounded (no unbounded growth on chatty runs).

## [0.1.0] — 2026-06-23

First public release.

### Added
- **New Job** for rclone and rsync: `copy` / `sync (mirror)` / `move`, with a
  live command preview, risk badge, dry-run + start, live progress (speed/ETA),
  cancel, and an Advanced section (include/exclude, transfers/checkers/bwlimit/
  retries, checksum, compress, SSH port, validated custom flags).
- **Backup Assistant** with guided scenarios.
- **Remotes**: add, delete and browse rclone remotes without typing paths.
- **Mounts**: mount a remote onto a local folder and manage active mounts.
- **Queue**: run up to N jobs in parallel, with pause, reorder, remove, cancel.
- **Scheduling**: export a job as a systemd user timer (no internal daemon).
- **Profiles**, **History** with severity-filtered logs, **Job Details**,
  **Settings**, and an optional local `rclone rcd` daemon.
- **Internationalization** (gettext) with a Romanian translation.
- **Packaging**: `.deb` (cargo-deb), desktop entry, AppStream metainfo, scalable
  icon, and a Flatpak manifest that bundles rclone and rsync.

### Security
- Commands are built as argument vectors — never shell strings (no injection).
- Secrets are sanitized from logs and the UI, and never persisted by the app.
- Path guards reject `/`, `$HOME`, `..` and symlinks that resolve to them.
- Destructive operations require confirmation; cancellation is graceful
  (SIGTERM before SIGKILL). Output is bounded to prevent OOM.
- systemd unit generation is hardened against specifier/newline injection.

[Unreleased]: https://github.com/Alexandru2984/rclone_gui/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/Alexandru2984/rclone_gui/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Alexandru2984/rclone_gui/releases/tag/v0.1.0
