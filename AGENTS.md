# HQCode Repository Guidelines

HQCode is a public MIT-licensed, self-hosted Rust control plane for persistent `tmux` sessions running Claude Code, Codex, OpenCode, and interactive shells.

## Repository Architecture & Shared Crates

- **Binary crate**: `src/main.rs` and `src/server.rs` provide a thin CLI and Axum HTTP/WebSocket control plane.
- **Shared workspace crates (`crates/*`)**:
  - `hq_fs_atomic`: atomic file writes and `0o600` owner-only JSON/secret persistence.
  - `hq_runtime_paths`: dynamic OS user/passwd/$HOME resolution and thread-safe local time helpers.
  - `hq_workspace`: directory browsing boundaries (`allow_one_parent`) plus Git status, diff, revert, and commit operations.
  - `hq_tmux`: isolated `tmux` session lifecycle, `runuser -u USER -- SHELL -l` spawning, input pacing, and `portable-pty` backlog/broadcast streaming.
  - `hq_auth`: Argon2id credentials, HMAC-SHA256 cookies (including device-bound claims), initial bootstrap password, and loopback Bearer tokens.
  - `hq_presets`: CLI binary detection (`claude`, `codex`, `opencode`, `bash`), real-config inspection/import, and `apply`/`run` transactions.
  - `hq_remote_connect`: LAN/Tailscale/public endpoint discovery, one-time QR pairing tickets, SVG/terminal QR rendering, and revocable paired-device registry.
- **Parameterization contract**: Keep shared crates free of application-specific ports, paths, cookie names, or `tmux` socket prefixes so HQCode and sibling applications can share them without runtime conflicts (HQCode defaults: bind `127.0.0.1:9900`, data dir `~/.hqcode`, `tmux` socket `hqcode_<session_id>`, cookie `hqcode_session`).

## Terminal I/O & Preset Semantics

- Terminal ownership is limited to creating/adopting the PTY/`tmux` session and forwarding terminal I/O bytes.
- New login shells must use `runuser -u USER -- SHELL -l` (never `--login`, which breaks `runuser -u` + `tmux` on target Linux hosts).
- Never inject or override `HOME`, `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, proxy env vars, auth env vars, or model bypass env vars into terminal sessions.
- `hqcode run` (temporary preset execution) writes to the native Codex/Claude configuration files, waits a short settle window for startup, and restores the previous files afterward; `hqcode use` applies persistently without restore.

## Security & Persistence Boundaries

- Persist credentials, tokens, and session metadata via `hq_fs_atomic` (`0o600` atomic writes).
- Preserve Argon2id password verification, HMAC-SHA256 session cookies, default loopback binding (`127.0.0.1:9900`), and `hq_workspace` path-boundary checks (`allow_one_parent`).

## Static Assets & Icons

- `static/` (`index.html`, `app.js`, `styles.css`, `favicon.ico`) is embedded into the binary with `include_dir!`, while disk files under `static/` take precedence when present at runtime.
- Keep root `icon.ico` and `static/favicon.ico` identical as 6-size transparent RGBA `.ico` files (`16x16` through `256x256`), served at `/favicon.ico` and `/static/favicon.ico`.

## Public Sanitization Gate

- Never commit `SKILL.md`, `.codex/skills/`, `.kiro/`, `.grok/`, internal hostnames, private keys, or `.hqcode-*` runtime/credential state files into this repository. `scripts/package-release.sh` enforces this sanitization gate before packaging releases.

## Verification & Commits

- Before committing Rust or frontend changes, verify all checks pass cleanly:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `node --test tests/frontend.test.mjs`
  - `python3 tests/e2e_browser.py` (when server or WebUI flows change)
- Use Conventional Commits (`feat:`, `fix:`, `docs:`, `chore:`) and stage only files touched by the current task.
