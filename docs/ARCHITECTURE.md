# Cascade — Architecture

> Cascade is a native, lightweight Linux GUI for **rclone** and **rsync**.
> Working product name — easy to rename (see README → "Renaming the project").

---

## A. Final technology recommendation

**Decision: Rust + GTK4 / libadwaita + Tokio + SQLite (rusqlite).**

### Why Rust + GTK4 over Go + GTK4

| Criterion | Rust + GTK4 (`gtk4-rs`) | Go + GTK4 (`gotk4`) |
|---|---|---|
| Binding maturity | Very mature, first-class, GNOME-endorsed | Younger, auto-generated, fewer real-world apps |
| Memory model for a *data-destroying* tool | Compile-time guarantees, exhaustive enums for state machines, `Result` everywhere | GC + `nil` panics, weaker compile-time guarantees |
| Resource footprint | No runtime, no GC pauses, small RSS | Small but has GC + goroutine runtime |
| Process/async | Tokio: best-in-class async process supervision | goroutines are simple, but binding interop is rougher |
| Portfolio signal | High (Rust + GTK4 is impressive and uncommon) | Medium |
| Cost | Steeper learning curve (`Rc`/`RefCell`, GObject), slower compiles | Faster to write, faster compiles |

For a tool whose **bugs delete user data**, Rust's type system is the deciding factor:
the job state machine, command builders, and destructive-operation gates are all encoded
as types the compiler forces us to handle. rsync is C and rclone is Go, but the GUI talks
to them over **process boundaries + the rclone RC HTTP API**, so the GUI language is free.

**Rejected:** Electron/React/Tauri/webview — heavy, non-native, defeats the "lightweight" goal.

### Trade-offs we accept
- Longer compile times and a harder learning curve (`Rc<RefCell<T>>`, GObject subclassing).
- We mitigate GObject pain by keeping **all business logic in a GTK-free `cascade-core` crate**
  that is 100% unit-testable without a display server.

---

## B. Architecture overview

Two-crate Cargo **workspace** enforcing the UI / logic split:

```
cascade-core   (no GTK, no display) ── pure logic, fully testable in CI
      ▲
      │  events (async-channel), commands (argv Vec<String>)
      │
cascade-gui    (GTK4 + libadwaita) ── thin view layer, no business rules
```

### Data flow (one job)

```
            ┌──────────────┐     build argv (Vec<String>)     ┌───────────────┐
 User  ───▶ │  GUI / View  │ ───────────────────────────────▶ │ Command Builder│
            └──────────────┘                                   │  rclone/rsync  │
                  ▲                                            └───────┬───────┘
                  │ ProcessEvent stream (async-channel)                │ argv
                  │ (Started/Stdout/Stderr/Progress/Finished)          ▼
            ┌─────┴────────┐    spawn (argv, NO shell)         ┌───────────────┐
            │  Job Queue   │ ◀──────────────────────────────── │ Process Runner │
            │ state machine│        events                     │  (Tokio child) │
            └─────┬────────┘                                   └───────┬───────┘
                  │ persist                                            │ stdout/stderr lines
                  ▼                                                    ▼
            ┌──────────────┐                                   ┌───────────────┐
            │   Storage    │                                   │ Log Sanitizer │ (redact secrets)
            │   (SQLite)   │                                   └───────────────┘
            └──────────────┘
```

### How jobs run
1. View collects user intent → an `RcloneOp` / `RsyncOp` options struct.
2. `security::path` validates source/destination; `security::destructive` flags risky ops.
3. Command builder turns the options struct into a `Vec<String>` **argv** (never a shell line).
4. `process::Runner` spawns the child with `tokio::process::Command` using the argv vector,
   `stdin = null`, capturing stdout+stderr line streams. It runs on a dedicated Tokio thread
   and emits `ProcessEvent`s over an `async-channel`.
5. The GUI consumes the channel with `glib::spawn_future_local` (async-channel is executor-agnostic,
   so it bridges Tokio ↔ GLib main loop cleanly) — UI never blocks.
6. The Job Queue updates the in-memory state machine and persists transitions to SQLite.
7. Every stdout/stderr line passes through the **log sanitizer** before display or storage.

### How we talk to rclone
- **Primary (Phase 2):** start a local `rclone rcd` daemon bound to `127.0.0.1:<random-port>`,
  protected by a random user/pass, and drive it over the **RC HTTP API** (rich JSON: stats,
  per-transfer progress, listings, remote management). Never exposed off-loopback.
- **Fallback (Phase 1 + always):** spawn `rclone <op>` as a CLI process and parse
  `--use-json-log` / `--stats` / `--progress` output. Used when RC is overkill or unavailable.

### How we talk to rsync
- Always CLI process. Use `--info=progress2 --outbuf=L` for parseable, line-buffered progress.
  SSH transport via rsync's own `-e ssh ...`; we build the `ssh` args as a vector too.

### Progress parsing
- **rclone:** prefer RC `core/stats` JSON, or CLI `--use-json-log` structured events.
- **rsync:** parse `--info=progress2` lines (percent, rate, ETA) with a tolerant regex;
  fall back to raw line passthrough when a line doesn't match.

### Persistence
- `rusqlite` with the **bundled** SQLite (no system `libsqlite3-dev` dependency).
- WAL mode, a `schema_version` table, forward-only migrations.
- See [SCHEMA.md](SCHEMA.md).

---

## C. Directory structure

```
gui_rclone/
├─ Cargo.toml                 # workspace
├─ README.md
├─ docs/
│  ├─ ARCHITECTURE.md         # this file (A,B,C,E)
│  ├─ THREAT_MODEL.md         # H
│  ├─ SCHEMA.md               # D
│  └─ ROADMAP.md              # F, G
├─ crates/
│  ├─ cascade-core/           # GTK-free business logic
│  │  ├─ Cargo.toml
│  │  ├─ examples/
│  │  │  └─ dry_run_job.rs     # runnable end-to-end demo (rsync dry-run)
│  │  └─ src/
│  │     ├─ lib.rs
│  │     ├─ error.rs
│  │     ├─ config.rs          # XDG paths, app config
│  │     ├─ security/
│  │     │  ├─ mod.rs
│  │     │  ├─ path.rs         # path validation, root-/ guard
│  │     │  ├─ destructive.rs  # destructive-op detection
│  │     │  └─ sanitize.rs     # log secret redaction
│  │     ├─ process/
│  │     │  └─ mod.rs          # async argv runner, ProcessEvent stream
│  │     ├─ rclone/
│  │     │  ├─ mod.rs
│  │     │  ├─ detect.rs       # binary detection/version
│  │     │  └─ command.rs      # argv builder + options
│  │     ├─ rsync/
│  │     │  ├─ mod.rs
│  │     │  ├─ detect.rs
│  │     │  └─ command.rs
│  │     ├─ job/
│  │     │  ├─ mod.rs          # Job model
│  │     │  └─ state.rs        # state machine + transitions
│  │     └─ storage/
│  │        ├─ mod.rs          # connection, migrations
│  │        └─ schema.rs       # DDL
│  └─ cascade-gui/            # GTK4 + libadwaita
│     ├─ Cargo.toml
│     └─ src/
│        ├─ main.rs
│        ├─ app.rs             # AdwApplication
│        └─ window.rs          # main window shell (sidebar + stack)
├─ packaging/                 # (Phase 5) .deb, AppImage, desktop file, icon
└─ .github/workflows/ci.yml   # (Phase 5) fmt + clippy + test + build
```

---

## E. UI / UX design

### Screens
`Dashboard · New Job · Backup Assistant · Remote Browser · Local Browser ·
Jobs Queue · Job Details · Logs · Profiles · Settings · Mounts · History`

Shell = `AdwApplicationWindow` with an `AdwNavigationSplitView`: sidebar list on the left,
an `AdwViewStack` of screens on the right. Dark/light follows the system via libadwaita
`AdwStyleManager` (free, no custom CSS needed).

### Key components
- **Command preview bar** — every job screen shows the exact generated argv, monospace,
  with a **Copy command** button. Demystifies the tool, builds trust.
- **Danger affordances** — destructive operations (`sync --delete`, `delete`, `purge`)
  render with a red `destructive-action` style class and require a confirm dialog that
  names what will be deleted and offers **"Run dry-run first"** as the default button.
- **Big visible Dry-run button** next to Start on every operation.
- **Live progress** — `AdwBanner`/progress bar + speed + ETA + bytes/files counters,
  fed by the `ProcessEvent` stream.
- **Logs panel** — live, with filter chips: Errors / Warnings / Info / Raw. Sanitized.
- **Desktop notification** on job completion (success/failure) via the freedesktop portal.

### Flow — beginner (Simple Mode)
1. Pick **Source** (local/remote browser, no typing paths).
2. Pick **Destination**.
3. Pick **Operation**: Copy / Sync / Backup / Mount — each with a one-line plain-language
   explanation ("Copy = add files, never deletes" vs "Sync = make B identical to A, **may delete**").
4. See the command preview. Press **Dry-run** (recommended) or **Start**.
5. Watch progress; get a notification; see the result in History.

### Flow — power user (Advanced Mode)
A toggle on the New Job screen reveals all important flags grouped in `AdwExpanderRow`s:
include/exclude patterns, `--transfers/--checkers/--checksum`, chunk size, `--bwlimit`,
`--retries`, delete behavior, dry-run, verbosity, and a **validated** custom-flags field
(parsed and shown in the preview before it can run). Power users can also load/save **Profiles**.
