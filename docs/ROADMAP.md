# F. Implementation plan (phases) & G. Risk analysis

## F. Phased plan

### Phase 1 — MVP ✅
- [x] Cargo workspace, GTK-free core, clean module structure
- [x] rclone/rsync detection
- [x] copy/sync command builders (argv, never shell) + tests
- [x] async process runner with live stdout/stderr streaming
- [x] log sanitizer + path validation + destructive detection + tests
- [x] job state machine + tests
- [x] SQLite init + schema + migrations
- [x] command preview + dry-run (runnable example)
- [x] GTK New Job screen wired to the runner, with History

### Phase 2 — rclone advanced ✅
- [x] Remote browser (lsjson) with no-typing path picking
- [x] `rclone rcd` local daemon (loopback, random credentials), queried via `rclone rc`
- [x] mount / unmount management screen
- [x] Profiles (save/load); `--transfers/--checkers/--retries/--bwlimit` (Advanced)

### Phase 3 — Backup Assistant ✅
- [x] Seeded scenario templates feeding the New Job flow
- [x] Auto dry-run recommendation, anti-disaster validations (risk + confirmation)

### Phase 4 — rsync advanced ✅
- [x] SSH transport (port option), include/exclude patterns
- [x] `--info=progress2` parsing, rsync via profiles + advanced flags

### Phase 5 — polish / portfolio 🚧
- [x] GitHub Actions CI (fmt/clippy/test/build + desktop/appstream validation)
- [x] Packaging: `.deb` (cargo-deb), desktop file, AppStream metainfo, icon, user install script
- [x] README with feature list and threat model
- [ ] Screenshots + demo video script
- [ ] AppImage build recipe wired into CI

---

## G. Risk analysis

### What can go wrong
- **Data deletion** via `sync --delete` / `delete` / `purge` in the wrong direction.
- **Overwrite** of newer files with older ones.
- **Wrong remote** selected (e.g. backing up *into* a source).
- **Mount left dangling** after a crash, blocking a mountpoint.
- **Credential exposure** in logs or screenshots.
- **UI freeze** if a child process is run on the UI thread (we never do this).

### How we prevent data loss (mandatory validations)
1. **No implicit deletes** — delete/purge flags only added when the user explicitly chose them
   *and* passed the confirmation gate.
2. **Path guards** — reject `/`, empty/whitespace, bare `$HOME`; warn on system dirs; require
   normalized, existing-or-explicit paths.
3. **Dry-run first** — the default action for any `caution`/`destructive` op is dry-run; the
   summary must be shown before a real run is allowed.
4. **Double confirmation** for destructive ops, with the target named in the dialog.
5. **Direction sanity** — warn when destination is a parent/child of source, or identical.
6. **Atomic state transitions** — the job state machine rejects illegal transitions, so a
   "completed" run can't silently resume and re-delete.
7. **Sanitized, persisted logs** — every run is auditable after the fact.

### Mandatory validations checklist (enforced in `cascade-core`, covered by tests)
- [x] `security::path::validate` — empty/root/home guards
- [x] `security::destructive::classify` — op risk level
- [x] `security::sanitize::redact` — secret redaction
- [x] `job::state::transition` — legal state machine only
- [x] command builders never emit a shell string; delete flags are opt-in
