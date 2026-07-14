# Security Policy

## Reporting a vulnerability

Please report security issues **privately** — do not open a public issue.

- Preferred: GitHub → **Security** tab → **Report a vulnerability** (private advisory).
- Or email the address in the repository's commit history.

We aim to acknowledge a report within a few days and to ship a fix promptly.

## Supported versions

Security fixes target the latest release.

## Security posture

Cascade is a tool that can overwrite and delete data and that handles cloud
credentials, so it is treated as security-sensitive. The full analysis is in
[docs/THREAT_MODEL.md](docs/THREAT_MODEL.md). The latest completed review is the
[2026-07-14 security audit](docs/SECURITY_AUDIT_2026-07-14.md). In brief:

- Commands are built as **argument vectors — never shell strings** (no injection).
- Safety-critical custom flags cannot override dry-run, deletion limits, config,
  logging, or endpoints; operands follow an option terminator.
- Secrets are sanitized from multiline output and rejected before argv, queue,
  profile, history, or schedule persistence. Inline remote credentials are not
  accepted by the GUI.
- Path guards reject `/`, `$HOME`, `..`, unsafe symlinks, dangerous overlaps, and
  changed warning snapshots; paths are resolved again immediately before launch.
- Destructive operations and live recurring schedules require explicit consent.
  Cancellation terminates the entire child process group; output, captures,
  queues, and GUI scrollback are bounded.
- The optional local `rclone rcd` daemon binds loopback only, with random
  credentials passed via the environment (not argv), and is disabled below
  rclone 1.73.5.
- Dependencies and Flatpak sources are lock/checksum-pinned. Release actions use
  immutable SHAs, least-privilege jobs, checksums, and provenance attestations.
- Dependencies are scanned in CI with cargo-audit and cargo-deny.
