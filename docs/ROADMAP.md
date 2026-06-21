# F. Implementation plan (phases) & G. Risk analysis

## F. Phased plan

### Phase 1 — MVP  ◀ this skeleton targets Phase 1
- [x] Cargo workspace, GTK-free core, clean module structure
- [x] rclone/rsync detection
- [x] copy/sync command builders (argv, never shell) + tests
- [x] async process runner with live stdout/stderr streaming
- [x] log sanitizer + path validation + destructive detection + tests
- [x] job state machine + tests
- [x] SQLite init + schema + migrations
- [x] command preview + dry-run (runnable example)
- [ ] GTK New Job screen wired to the runner (window shell provided; wiring next)

### Phase 2 — rclone advanced
- Remote browser (ls/lsd/lsjson), local browser
- `rclone rcd` local daemon + RC HTTP client (loopback, random auth)
- mount / unmount management screen
- Profiles (save/load); `--transfers/--checkers/--retries/--bwlimit`

### Phase 3 — Backup Assistant
- Seeded scenario templates, Simple Mode flow
- Auto dry-run recommendation, anti-disaster validations

### Phase 4 — rsync advanced
- SSH transport, include/exclude pattern editor
- Better `--info=progress2` parsing, rsync profiles

### Phase 5 — polish / portfolio
- Final UI, screenshots, demo script
- Full test suite, GitHub Actions CI (fmt/clippy/test/build)
- Packaging: `.deb` + AppImage, desktop file, dependency checker
- README with screenshots, threat model, demo video script

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
