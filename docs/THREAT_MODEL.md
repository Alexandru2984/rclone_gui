# Local threat model

Cascade can overwrite or delete user data and invokes tools that handle cloud
credentials. It is security-sensitive even though it is a local desktop app.

## Trust boundaries

`user intent → GUI confirmation → JobSpec validation → argv/RC request →
rclone/rsync/systemd → local filesystem and remote services`

The local OS account, installed `rclone`/`rsync` binaries, the user's rclone
configuration, and configured remote services are trusted. Text entered into job
fields, restored database rows, child-process output, filesystem paths, symlinks,
and downloaded build inputs are treated as untrusted.

## Threats and controls

| # | Threat | Controls |
|---|---|---|
| 1 | Shell or option injection | Cascade never invokes a shell. Commands use explicit argv vectors with `stdin = null`. Source/destination operands follow `--`; control characters and option-like endpoints are rejected. Custom flags must be self-contained long options, and flags that can override dry-run, deletion limits, config, logging, remote execution, or endpoints are forbidden. |
| 2 | Credentials reaching argv or persistence | `JobSpec::ensure_no_embedded_secrets` runs before preview, argv construction, queue/profile/history storage, and execution. Credential-bearing `rclone config create` parameters are blocked because rclone would expose them in `/proc/<pid>/cmdline`; OAuth or interactive `rclone config` must be used instead. RC credentials are random per session and passed through environment variables. |
| 3 | Secret leakage through output | Streaming output passes through a stateful sanitizer before it leaves the process module. It redacts provider secrets, credential URLs, authorization headers, token JSON, and complete multiline PEM private keys. Captured output, previews, errors, historical free-form rows, and log writes are sanitized again at their persistence boundaries. |
| 4 | Destructive operation without informed consent | Risk classification is derived from the typed operation and validated options. Destructive and overlap warnings enter the GUI confirmation gate; dry-run is the safe default. Typed `max-delete` and `backup-dir` controls cannot be negated by custom flags. Queueing uses the same gate. |
| 5 | Catastrophic, overlapping, or changed paths | Empty/root/home paths, traversal, control characters, wrong-tool endpoint forms, and unsafe backup directories are rejected. Existing local paths and the nearest existing ancestor of new destinations are canonicalized. Root/system/remote-root/overlap warnings require acknowledgement. Immediately before execution, paths are resolved again and the warning snapshot must still match. Restored queue items with warnings remain blocked until reviewed. |
| 6 | Vulnerable or exposed rclone RC daemon | RC is disabled unless rclone is parseably versioned at 1.73.5 or newer, which contains the fixes for CVE-2026-41176 and CVE-2026-41179. The daemon binds only `127.0.0.1` on a random free port and requires CSPRNG-generated credentials; `--rc-no-auth` is never used. Paths are revalidated after daemon startup and before submission. |
| 7 | Memory exhaustion, hung children, or orphaned descendants | Streaming lines are capped at 64 KiB, event queues at 128 entries, GUI/log views are bounded, and captures are limited to 16 MiB per stream with a 120-second timeout. Children run in dedicated process groups. Cancellation sends SIGTERM and then SIGKILL to the group; timeout and RC teardown also terminate descendants. Parallel jobs are capped at eight. |
| 8 | Sensitive or attacker-controlled files at rest | XDG application directories are real, private directories rather than symlinks. SQLite and logs are mode `0600`; directories are `0700`; new logs use exclusive creation and refuse symlinks. SQLite enables `secure_delete`, purges legacy credential-bearing rows at startup, and re-sanitizes historical text. Log readers enforce containment and size limits. |
| 9 | Unattended destructive schedules | New schedules are dry-run by default. Every live recurring schedule requires an explicit acknowledgement that no future prompt will appear. Paths and warnings are revalidated, executables are canonicalized, `OnCalendar` uses a strict bounded grammar, and systemd arguments are quoted. Existing timers are stopped before replacement. Unit files are mode `0600`, staged with exclusive creation, atomically renamed, and never written through symlinks. |
| 10 | Persisted work running with stale intent | A restored queue starts paused. Stored specs are revalidated and resolved immediately before launch; unacknowledged or changed warnings block execution. Credential-bearing rows are rejected at both serialization and repository boundaries. |
| 11 | Supply-chain substitution | Rust and GitHub Actions are pinned to exact versions/commit SHAs. Release download URLs are immutable and SHA-256 verified. Build jobs have read-only tokens; only the isolated publish job can write releases. Releases include `SHA256SUMS` and GitHub provenance attestations. Flatpak archives and every Cargo crate are checksum-pinned, and the Cargo source list is deterministically checked against `Cargo.lock`. |
| 12 | Unnecessary privilege or sandbox escape | Cascade never invokes `sudo`. The Flatpak uses a current GNOME runtime and grants only the network, notifications, display, GPU, and home access required for a backup tool; unused Secret Service access was removed. Native packages run with the user's normal permissions. |

## Residual risks

- A filesystem object can still be replaced while an external transfer is in
  progress. Cascade narrows the check/use window by resolving immediately before
  launch, but cannot freeze a live filesystem; use snapshots for hostile or
  concurrently mutating trees.
- Sanitization recognizes credential structures and provider naming conventions;
  no heuristic can identify an arbitrary random secret stored under an innocent
  field name. Do not put credentials in paths, labels, include/exclude patterns,
  or custom fields.
- Native builds intentionally trust the selected `rclone` and `rsync` binaries,
  the user's `PATH`, rclone configuration, remote provider, OS, and GitHub runner
  image. Checksums and provenance establish what was built, not that every
  upstream component is defect-free.
- Flatpak requires broad home-directory access because its purpose is to back up
  arbitrary user files. This is a functional permission, not a containment
  boundary for user data.

## Non-goals

- Defending against an attacker who already controls the same OS account.
- Replacing filesystem snapshots, remote-side versioning, or independent backups.
- Managing, recovering, or cryptographically protecting rclone's own config; that
  file belongs to rclone and should be protected with its supported mechanisms.
