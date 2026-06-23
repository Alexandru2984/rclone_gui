## Summary

What does this change and why?

## Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test -p cascade-core` passes
- [ ] New core logic has tests
- [ ] No secrets are logged or persisted; user output is sanitized
- [ ] New user-facing strings are wrapped for translation (`tr` / `n`)
- [ ] CHANGELOG.md updated (under "Unreleased")
