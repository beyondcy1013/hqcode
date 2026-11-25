# Changelog

## v0.1.0 (2026-11-25)

- Initial public MIT release of **HQCode**.
- Parameterized shared workspace crates: `hq_fs_atomic`, `hq_runtime_paths`, `hq_workspace`, `hq_tmux`, `hq_auth`, `hq_presets`.
- Persistent `tmux` session engine with `runuser -u USER -- SHELL -l`, 5ms erase-key pacing, backlog replay, and cross-session output search.
- Lightweight Git Review drawer (`status`, line-numbered unified `diff`, `revert`, `commit`) with automatic active-session directory tracking.
- Responsive dark WebUI with EN/中文 language toggle, session tabs, inline rename, `visualViewport`-aware mobile soft-key bar, and zero external frontend build step.
- One-line `install.sh`, release packaging script, GitHub Actions CI/Release workflows, Dockerfile, and systemd unit template.
