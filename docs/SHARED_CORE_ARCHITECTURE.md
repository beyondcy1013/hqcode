# Shared Core Architecture & Multi-Instance Isolation

HQCode is structured as a thin binary (`src/`) on top of **7 parameterized workspace crates** (`crates/*`) so both **HQCode** (public MIT control plane) and **`webClx`** (private heavy-ops control plane) can share the same foundational engine without code duplication or runtime conflicts.

## Shared Crates (`crates/*`)

| Crate | Responsibility | Key Parameterization / Contract |
| --- | --- | --- |
| `hq_fs_atomic` | Atomic writes & `0o600` owner-only JSON/secret persistence | Path-based; zero app-specific constants |
| `hq_runtime_paths` | Unix `passwd` / `$HOME` dynamic user resolution & thread-safe local time | `resolve_data_dir(env_var, dir_name)`, `resolve_workspace_dir(env_var)` |
| `hq_workspace` | Workspace path boundary (`allow_one_parent`) + Git status/diff/revert/commit | Accepts any `WorkspaceConfig` |
| `hq_tmux` | Isolated `tmux` lifecycle, `runuser -u USER -- SHELL -l`, 5ms erase-key pacing, `portable-pty` backlog & broadcast | Parameterized via `TmuxNamespace { socket_prefix, session_prefix, scope_unit_prefix, after_service }` |
| `hq_auth` | Argon2id credentials, HMAC-SHA256 cookies (incl. device-bound claims), initial bootstrap password, loopback Bearer token | Parameterized via `AuthConfig { cookie_name, file_prefix, default_username, session_ttl_days }` |
| `hq_presets` | CLI binary detection (`claude`, `codex`, `opencode`, `bash`), real-config inspection/import & `apply`/`run` transactions | Parameterized store file prefix; zero `CODEX_HOME`/`CLAUDE_CONFIG_DIR` env injection |
| `hq_remote_connect` | LAN/Tailscale/public endpoint discovery, one-time QR pairing tickets, SVG/terminal QR rendering & revocable paired-device registry | Parameterized via `RemoteConnectConfig { file_prefix, pair_path, default_ttl_secs, max_active_tickets }` |

## Side-by-Side Runtime Isolation (`HQCode` + `webClx`)

When running HQCode and `webClx` simultaneously on the same Linux workstation:

| Boundary | `HQCode` | `webClx` |
| --- | --- | --- |
| Default Port | `127.0.0.1:9900` (override via `--addr` / `HQCODE_ADDR`) | `11111` (configured service port) |
| Data Directory | `~/.hqcode` (override via `--data-dir` / `HQCODE_DATA_DIR`) | `/home/bin/webclx` |
| `tmux` Socket | `tmux -L hqcode_<session_id>` | `tmux -L webclx_<session_id>` |
| Session Cookie | `hqcode_session` | `webclx_session` |
| State Files | `.hqcode-credentials.json`, `hqcode-sessions.json` | `.webclx-*` |

## Reusing `crates/*` from `webClx`

In `/home/codes/tools/webClx/Cargo.toml`, shared crates can be referenced by relative path:

```toml
hq_fs_atomic = { path = "../hqcode/crates/hq_fs_atomic" }
hq_runtime_paths = { path = "../hqcode/crates/hq_runtime_paths" }
hq_workspace = { path = "../hqcode/crates/hq_workspace" }
hq_tmux = { path = "../hqcode/crates/hq_tmux" }
```

`webClx` passes `TmuxNamespace::new("webclx")` and `AuthConfig::for_app("webclx")` to retain 100% backward compatibility with existing sessions and cookies.
