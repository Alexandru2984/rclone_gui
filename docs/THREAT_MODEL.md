# H. Local threat model

Cascade is a tool that can **overwrite and delete user data** and handle cloud credentials.
We treat it as security-sensitive even though it is a local desktop app. Trust boundaries:
the **user's intent** (UI) → **command construction** (core) → **child processes** (rclone/rsync/ssh)
→ **remote services & the filesystem**.

| # | Threat | Vector | Mitigation (where) |
|---|---|---|---|
| 1 | **Shell injection** | A path/remote/pattern like `; rm -rf ~` interpreted by a shell | Never invoke a shell. Spawn with `tokio::process::Command` and an **argv vector**; `stdin = null`. No `sh -c`, no string concatenation of commands. (`process/`, `*/command.rs`) |
| 2 | **Secret leakage in logs** | rclone/rsync/ssh echo tokens, `--password`, OAuth blobs, credential URLs, SSH keys | Mandatory `security::sanitize` pass on **every** line before display **and** before disk write. Redacts known flag values, `token:{...}` JSON, `user:pass@host` URLs, `Authorization:` headers, PEM key bodies. (`security/sanitize.rs`) |
| 3 | **Destructive sync / data loss** | `sync`/`--delete`/`purge`/`delete` silently removing files; wrong direction | `security::destructive` classifies every op; UI requires a **double confirmation** naming the target, defaults to **dry-run first**, and shows a red danger style. Delete flags are never added implicitly. (`security/destructive.rs`, GUI) |
| 4 | **rclone RC exposure** | `rcd` reachable from the network / unauthenticated | Bind **only** `127.0.0.1` on a random high port; generate a random user + pass per session; pass `--rc-no-auth` is **forbidden**; never advertise the port. (`rclone/` RC client, Phase 2) |
| 5 | **Malicious / catastrophic paths** | Selecting `/`, `~`, an empty/whitespace destination, or symlink traversal as src/dst | `security::path` rejects empty paths, the filesystem root `/`, and bare `$HOME`; warns on system dirs; requires non-empty, normalized paths before a job can be built. (`security/path.rs`) |
| 6 | **Log file leakage at rest** | Sanitized-but-sensitive logs world-readable | Logs written under XDG data dir with `0700` dir / `0600` file perms; only metadata in SQLite. (`config.rs`, `logs/`) |
| 7 | **Privilege escalation** | Running rclone/rsync as root | App **never** calls `sudo`. If an operation genuinely needs elevation (e.g. some mounts), we **print the exact manual command** for the user to run themselves. |
| 8 | **Credential storage** | Tokens/passwords in plaintext SQLite/config | Secrets go to the **Secret Service / keyring**; SQLite stores only a key reference. We reference rclone's existing native config rather than duplicating its secrets. |
| 9 | **Accidental mass overwrite** | Large destructive overwrite without awareness | Pre-flight `--dry-run` summary surfaced to the user; "massive overwrite" heuristic triggers the same confirm gate as delete. |
| 10 | **Untrusted custom flags** | Power-user free-text flags injecting behavior | Custom flags are tokenized into individual argv items (no shell), validated against an allow-pattern, and shown in the preview before the run is permitted. |

## Non-goals (explicit)
- Not a sandbox/MAC layer — we rely on the OS user's own filesystem permissions.
- We do not attempt to defeat a malicious *local* user who already controls the account.
