# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/Alexandru2984/rclone_gui/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Alexandru2984/rclone_gui/releases/tag/v0.1.0
