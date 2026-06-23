# Cascade 0.1.0

A native, lightweight GTK4 / libadwaita desktop GUI for **rclone** and **rsync** — no Electron.

## Highlights
- **Both tools**: copy / sync (mirror) / move, built as argument vectors (never a shell string)
- **New Job**: tool & operation pickers, file/folder choosers, live command preview, risk badge,
  dry-run + start, live progress (speed/ETA), cancel, and an Advanced section
  (include/exclude, transfers/checkers/bwlimit/retries, checksum, compress, SSH port, custom flags)
- **Backup Assistant**: guided scenarios (photos, projects, Google Drive, VPS over SSH, mirror, restore)
- **Remotes**: add/delete and browse rclone remotes without typing paths
- **Mounts**: mount a remote onto a local folder and manage active mounts
- **Queue**: run up to N jobs in parallel, with pause, reorder, remove and cancel
- **Scheduling**: export any job as a systemd user timer (no internal daemon)
- **Profiles / History / Job Details**: save & reload jobs; review past runs with filtered logs
- **Settings**: light/dark/system theme, parallelism, destructive-confirmation toggle
- **i18n**: full gettext-based translation; Romanian included

## Safety
- No shell, no SQL injection, secrets sanitized from logs/UI, never stored by the app
- Path guards (root/$HOME/symlink/`..`), destructive-op confirmation, graceful cancel
- Optional local-only rclone RC daemon (loopback + random credentials via env)

## Install
- Debian/Ubuntu: install the attached `.deb`
- From source: `cargo build -p cascade-gui --release` (needs `libgtk-4-dev libadwaita-1-dev`)
- Runtime tools: `rclone`, `rsync`

Requires a Linux desktop with GTK4 ≥ 4.12 and libadwaita ≥ 1.5. Runs on Wayland and X11.
