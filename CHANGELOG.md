# Changelog

## v0.2.0 (2026-11-25)

- Added shared `hq_remote_connect` workspace crate: endpoint auto-discovery (`Public` / `Tailscale` / `LAN` / `Loopback`), one-time pairing tickets (`GET /pair` shell + `POST /api/remote/redeem` anti-prefetch burn), terminal + SVG QR rendering, and `0o600` paired-device registry with instant Cookie revocation (`401`).
- Extended `hq_auth` with device-bound `SessionClaims` (backward-compatible with legacy session cookies) and direct password reset support.
- Added `hqcode serve --lan` / `--public-url` / `--no-qr`, startup diagnostic + scannable terminal QR card, and `hqcode doctor`, `hqcode passwd`, `hqcode pair` CLI subcommands.
- Upgraded desktop and mobile UX: `#connect-modal` remote QR pairing & device management, mobile session selector (`#mobile-session-select`), compact non-overlapping mobile topbar, Enter-to-send quick input (`#mobile-quick-input`), `^D` / arrow mobile soft keys, 3-second two-click confirmation for session close and Git revert, sub-project Quick Launch cwd tracking, and 1-click import/edit of local `~/.codex` and `~/.claude` configs in `hq_presets`.
- Updated GitHub Actions `.github/workflows/release.yml` to use `macos-latest` and portable SHA-256 checksums for all 4 release targets (`linux-x86_64`, `linux-aarch64`, `macos-aarch64`, `macos-x86_64`).

## v0.1.0 (2026-11-25)

- Initial public MIT release of **HQCode**.
- Parameterized shared workspace crates: `hq_fs_atomic`, `hq_runtime_paths`, `hq_workspace`, `hq_tmux`, `hq_auth`, `hq_presets`.
- Persistent `tmux` session engine with `runuser -u USER -- SHELL -l`, 5ms erase-key pacing, backlog replay, and cross-session output search.
- Lightweight Git Review drawer (`status`, line-numbered unified `diff`, `revert`, `commit`) with automatic active-session directory tracking.
- Responsive dark WebUI with EN/中文 language toggle, session tabs, inline rename, `visualViewport`-aware mobile soft-key bar, and zero external frontend build step.
- One-line `install.sh`, release packaging script, GitHub Actions CI/Release workflows, Dockerfile, and systemd unit template.
