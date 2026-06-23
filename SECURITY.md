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
[docs/THREAT_MODEL.md](docs/THREAT_MODEL.md). In brief:

- Commands are built as **argument vectors — never shell strings** (no injection).
- Secrets are **sanitized** from logs and the UI, and the app **never stores
  credentials** — it refuses to save a profile that embeds one.
- Path guards reject `/`, `$HOME`, `..`, and symlinks that resolve to them.
- Destructive operations require confirmation; cancellation is graceful
  (SIGTERM then SIGKILL); process output is length-bounded to prevent OOM.
- The optional local `rclone rcd` daemon binds loopback only, with random
  credentials passed via the environment (not argv).
- Dependencies are scanned in CI (cargo-audit and cargo-deny).
