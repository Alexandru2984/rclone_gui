# Contributing to Cascade

Thanks for your interest! Cascade is a native GTK4 / libadwaita GUI for rclone
and rsync, written in Rust.

## Project layout

A Cargo workspace with a hard split between logic and UI:

- [`crates/cascade-core`](crates/cascade-core) — all business logic, **no GTK**.
  Command builders, the process runner, storage, security, scheduling. Fully
  unit-testable without a display.
- [`crates/cascade-gui`](crates/cascade-gui) — the GTK4 view layer on top.

The golden rule: **business logic never touches GTK**, and **commands are never
built as shell strings** — always argument vectors.

## Building and running

```bash
sudo apt install libgtk-4-dev libadwaita-1-dev   # build deps
cargo run -p cascade-gui                          # run the app
cargo run -p cascade-core --example dry_run_job   # core demo, no display
```

## Before opening a PR

All of these must pass (CI enforces them):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p cascade-core            # unit + integration tests
```

Please:

- Add tests for new core logic (command builders, parsers, state machines…).
- Keep secrets out of logs and the database — route user output through
  `security::sanitize`, and never persist credentials.
- Wrap user-facing strings in `i18n::tr(...)` (GUI) or mark core data literals
  with `cascade_core::n(...)`. See [Translations](#translations).
- Write a short, imperative commit subject (e.g. "add mount progress parsing").

## Translations

```bash
sudo apt install gettext
po/extract.sh        # regenerate po/cascade.pot from the source
po/build-mo.sh       # compile *.po into po/locale/<lang>/LC_MESSAGES/cascade.mo
```

Add a language by copying `po/cascade.pot` to `po/<lang>.po`, translating the
`msgstr`s, and rebuilding.

## Reporting issues

Use the issue templates. Include your distro, GTK/libadwaita versions, and the
exact command preview shown in the app (it is already secret-sanitized).
