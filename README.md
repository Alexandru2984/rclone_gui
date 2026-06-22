# Cascade

> A native, lightweight **GTK4 / libadwaita** desktop GUI for **rclone** and **rsync** on Linux.
> No Electron, no web stack — a real native app in Rust.

Cascade gives backups, syncs, copies, mounts and remote management a clear, safe
interface — friendly enough for normal users, powerful enough for power users —
while never hiding the real `rclone`/`rsync` command it runs.

> **Name:** "Cascade" is a working name. To rename, change `APP_ID`/`APP_NAME` in
> [`crates/cascade-core/src/config.rs`](crates/cascade-core/src/config.rs) and the crate names.

---

## ⚠️ Destructive-operation warning

Cascade can **overwrite and delete data**. It is built defensively, but you are
responsible for what you run. Cascade's safety guarantees:

- **Dry-run first** is the default for any risky operation.
- **Double confirmation** is required for destructive ops (`sync --delete`, `delete`, `purge`).
- Delete flags are **never** added implicitly.
- The filesystem root `/`, a bare `$HOME`, and empty paths are **refused**.
- Commands are built as **argument vectors — never shell strings** (no injection).
- Logs are **sanitized** of secrets before display or storage.
- Cascade **never uses `sudo`**; if elevation is truly needed, it shows you the manual command.

---

## Features (MVP / Phase 1 — in this repo)

- Detect `rclone` and `rsync` and show versions.
- Build safe `copy`/`sync` commands for both tools (argv, tested).
- Async process runner with **live, sanitized** stdout/stderr streaming (UI never blocks).
- Path validation, destructive-op classification, secret redaction — all unit-tested.
- Job state machine (pending → running → completed/failed/cancelled).
- SQLite (bundled) job history / profiles / settings store with migrations.
- Command **preview** and **dry-run** everywhere.

See [docs/ROADMAP.md](docs/ROADMAP.md) for Phases 2–5 (remote browser, rclone RC daemon,
mounts, Backup Assistant, SSH rsync, packaging).

---

## Architecture (short)

Two-crate workspace enforcing a hard UI / logic split:

| Crate | Role |
|---|---|
| [`cascade-core`](crates/cascade-core) | **No GTK.** Command builders, process runner, storage, security, job model. 100% unit-testable in CI. |
| [`cascade-gui`](crates/cascade-gui) | Thin GTK4 / libadwaita view layer. Bridges core's async events into the GLib loop. |

Full design: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) ·
[docs/SCHEMA.md](docs/SCHEMA.md) · [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md).

---

## Installing dependencies (Ubuntu 24.04 / 26.04)

Cascade needs the Rust toolchain plus the GTK4 + libadwaita **development** libraries.
SQLite is compiled in (`rusqlite` bundled) — no system SQLite needed.

```bash
# Build dependencies
sudo apt update
sudo apt install -y libgtk-4-dev libadwaita-1-dev build-essential

# Runtime tools Cascade drives
sudo apt install -y rsync
curl https://rclone.org/install.sh | sudo bash   # or: sudo apt install rclone

# Rust toolchain (if missing)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

---

## Running locally

```bash
# Core library: build + run the full test suite (no display needed)
cargo test -p cascade-core

# Safe end-to-end demo: rsync dry-run through the whole pipeline
cargo run -p cascade-core --example dry_run_job

# The GUI (needs libgtk-4-dev + libadwaita-1-dev installed)
cargo run -p cascade-gui
```

---

## Display servers: Wayland & X11

Cascade is GTK4/GDK, so it runs natively on **both Wayland and X11** with no code
differences — GDK picks the backend automatically. To force one for testing:

```bash
GDK_BACKEND=wayland cargo run -p cascade-gui
GDK_BACKEND=x11     cargo run -p cascade-gui
```

For **desktop notifications** and the **app icon** to appear correctly on either
backend, the app must be identified by an installed `.desktop` file whose name
matches the application id (`io.github.alexmihai.Cascade`). Running the raw
`target/debug/cascade` binary without installing it shows a generic icon/name;
install it (below) for the full, identical experience on both.

---

## Install & packaging

### Quick user install (no root) — recommended for trying it
```bash
packaging/install-local.sh            # build + install under ~/.local
packaging/install-local.sh --uninstall
```
Installs the binary, `.desktop`, icon and AppStream metainfo under `~/.local`,
so it shows up in your app menu with a proper icon and working notifications.

### Debian / Ubuntu `.deb`
```bash
cargo install cargo-deb        # once
cargo deb -p cascade-gui       # produces target/debian/cascade_0.1.0_*.deb
sudo apt install ./target/debian/cascade_*.deb
```
`rclone` and `rsync` are listed as **Recommends** (runtime tools), GTK/libadwaita
runtime libraries are auto-detected.

Packaging assets live in [`packaging/`](packaging/): the desktop entry, the
AppStream metainfo, and the scalable icon.

---

## Roadmap

`Phase 1 MVP` ✅ (this repo) → `Phase 2 rclone advanced` (remote browser, local RC
daemon, mounts, profiles) → `Phase 3 Backup Assistant` → `Phase 4 rsync advanced`
(SSH, include/exclude) → `Phase 5 polish` (CI, `.deb`/AppImage, screenshots).
Details: [docs/ROADMAP.md](docs/ROADMAP.md).

---

## License

GPL-3.0-or-later.
