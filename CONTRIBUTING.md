# Contributing to HQCode

Thank you for contributing to HQCode!

## Core Design Principles

1. **Terminal I/O Only (Zero Injection)**: HQCode creates/attaches isolated `tmux` PTY sessions and relays terminal I/O. Never inject `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, proxy overrides, or wrapper prompts into spawned agent terminals.
2. **Shared Parameterized Crates (`crates/*`)**: Keep `crates/*` free of hardcoded app names, ports, or socket prefixes so downstream control planes can reuse them safely.
3. **Zero Build Frontend**: Keep `static/` plain ES modules + CSS without requiring Node bundlers at runtime.

## Local Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node --test tests/frontend.test.mjs
```

## Pull Request Checklist

- Add or update unit tests for Rust crates and Node helpers.
- Ensure any process-spawning test cleans up child processes and `tmux` sockets via RAII `Drop` guards.
- Keep commits focused and descriptive.
