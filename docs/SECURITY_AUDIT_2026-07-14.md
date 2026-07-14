# Security audit — 2026-07-14

## Scope

The audit covered both Rust crates, command/RC construction, filesystem and path
handling, process lifecycle, SQLite/log persistence, queues, systemd scheduling,
Flatpak permissions and inputs, and all GitHub Actions workflows.

## Closed findings

| Severity | Finding | Resolution | Commit |
|---|---|---|---|
| Critical | RC daemon allowed rclone versions affected by pre-auth command execution | Fail-closed minimum version 1.73.5; Flatpak rclone updated and hashed | `ba3aab5` |
| High | Custom flags could override dry-run and deletion guards; endpoints could be parsed as options | Controlled flags forbidden; only self-contained long options accepted; operands follow `--` | `365f8b0` |
| High | Credentials could reach persisted job JSON and multiline private-key bodies could evade line redaction | Credential rejection at command/storage boundaries, legacy purge, stateful PEM redaction | `45b34d3` |
| High | Paths and warning acknowledgements could become stale before queued/RC execution | Canonical resolution and exact warning-snapshot revalidation at the execution boundary | `5f49b09` |
| High | Scheduling bypassed the destructive-operation gate | Dry-run default, explicit live consent, path snapshot, strict calendar grammar, safe unit replacement | `685f1b4` |
| High | Release workflows executed mutable downloads/actions with broad write permissions | Immutable SHAs/checksums, read-only build jobs, isolated publisher, checksums and attestations | `a5e2a5b` |
| Medium | Streaming/capture output and descendant processes lacked complete resource/lifecycle bounds | Bounded queues and output, capture timeouts, process groups, descendant termination | `0061b58` |
| Medium | Add Remote could expose provider credentials in the process list | Credential-bearing configuration is rejected before argv construction | `b6e5ef2` |
| Medium | Dependency graph included an older bundled SQLite stack and unused features/duplicates | Current minimal bundled SQLite/rusqlite graph and narrowed license policy | `7f76baf` |

No unresolved Critical, High, or Medium finding remains within the stated threat
model. Residual limitations are documented in [THREAT_MODEL.md](THREAT_MODEL.md).

## Verification

- `cargo test --locked --workspace --all-targets`: 327 core unit, 12 end-to-end,
  12 property, and 5 GUI tests passed.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: passed.
- `cargo audit --deny warnings`: 167 locked dependencies, no advisory or warning.
- `cargo deny check advisories bans licenses sources`: all four checks passed;
  one target-specific `windows-sys` duplicate remains in upstream GTK/development
  dependency branches and is informational.
- `actionlint` 1.7.7: all workflows passed.
- Every workflow action reference was independently checked to be a full 40-character
  commit SHA; mutable release/script references were removed.
- The deterministic Flatpak source list matches `Cargo.lock`; downloaded rclone,
  linuxdeploy, and GTK plugin hashes were checked against their pinned inputs.
