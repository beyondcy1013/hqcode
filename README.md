# HQCode

> **Your Coding Agents' Headquarters.** Keep native **Claude Code**, **Codex**, **OpenCode**, and shell sessions alive in `tmux` — controlled from desktop or phone in a single Rust binary.

*Built in Hengqin (HQ), built to be your Agents' HQ.*

## Why HQCode?

Most AI coding control surfaces (Electron / Tauri desktop wrappers) couple your active coding session to a fragile desktop bridge or wrap the CLI inside a custom chat UI. When you step away from your desk, restart the app, or switch to your phone, sessions disconnect or lag behind upstream CLI features.

**HQCode** takes a server-first, **Terminal I/O Only** approach:

- **Single Rust Binary (`Axum + Tokio`)**: Zero Node/Electron runtime overhead; embedded zero-build web UI.
- **Persistent Native `tmux` Sessions**: Close your browser, switch devices, or restart HQCode — your live Claude Code / Codex processes stay running inside isolated `tmux` sockets (`systemd --scope` when available).
- **Zero-Injection Fidelity**: Never overrides `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, or injects wrapper prompts. Upstream CLI features work on day zero.
- **Mobile-Ready Oversight**: Dedicated soft-key bar (`Esc`, `Tab`, `^C`, `↑`, `↓`, `y`, `Enter`), `visualViewport`-aware layout, and backlog replay without scroll jumping.
- **Lightweight Git Review Drawer**: Inspect `git status`, read colorized unified diffs, revert individual files, or commit right next to your terminal.
- **Real-Config Preset Transactions**: `hqcode use` and `hqcode run` switch real shared `~/.codex` and `~/.claude` configs cleanly.

## Quick Start

```bash
cargo build --release
./target/release/hqcode serve --addr 127.0.0.1:9900 --workspace /path/to/your/repo
```

On first launch, HQCode generates an owner-only (`0600`) random bootstrap password at `~/.hqcode/.hqcode-initial-password` (deleted automatically after your first successful login).

### CLI Preset Switching

```bash
hqcode list
hqcode use "Claude Sonnet"
hqcode run "Codex Fast" -- codex
```

## Shared Core Architecture

HQCode's core crates under `crates/` are parameterized and reusable across compatible control planes (such as `webClx`). See [docs/SHARED_CORE_ARCHITECTURE.md](docs/SHARED_CORE_ARCHITECTURE.md).

## License

MIT — see [LICENSE](LICENSE).
