# HQCode Launch & Community Playbook

> Ready-to-publish launch assets for pushing **HQCode v0.1.0** (`MIT`) and driving Day-7 GitHub Stars.

---

## 1. Pre-Publish GitHub Push & Release Checklist

Run these commands once you have created the empty GitHub repository `beyondcy1013/hqcode`:

```bash
cd /home/codes/tools/hqcode

# Verify clean working tree & passing checks first
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
node --test tests/frontend.test.mjs

# Push main & trigger multi-platform Release build via v0.1.0 tag
git remote add origin git@github.com:beyondcy1013/hqcode.git
git push -u origin main
git tag v0.1.0
git push origin v0.1.0
```

---

## 2. 60-Second Demo Video Storyboard

| Time | Action on Screen | Key Message |
| --- | --- | --- |
| **00s – 15s** | Start native `claude` in HQCode on desktop → close browser tab & restart `hqcode` → open the same URL on mobile browser | Same live terminal resumes instantly; zero dropped output |
| **15s – 35s** | Tap `y` / `↵` or send a short steer on `#hq-mobile-keys` → slide open **Git Diff** drawer to inspect line-numbered changes | Real mobile oversight + code review without switching apps |
| **35s – 52s** | Open a second tab in the same workspace running `codex` (or commit directly from the Diff drawer) | Multi-agent native workflow in one lightweight HQ |
| **52s – 60s** | Show `ls -lh target/release/hqcode` (~7.9 MB) and RSS (~33 MB) vs heavy Electron wrappers | Single Rust binary, MIT licensed |

---

## 3. Ready-to-Post Community Copy

### Show HN (Hacker News)
**Title:**
`Show HN: HQCode – Single Rust binary to keep native Claude Code/Codex alive in tmux`

**Body:**
> Hey HN, I built HQCode (MIT, Rust + Axum + tmux) because I got tired of desktop agent wrappers dropping sessions whenever I stepped away from my desk or switched to my phone.
>
> Instead of wrapping Claude Code or Codex inside an Electron/Tauri chat UI—or injecting custom `CODEX_HOME`/`CLAUDE_CONFIG_DIR` overrides—HQCode is strictly **Terminal I/O Only**:
> - Each session lives in an isolated `tmux` socket (`systemd --scope` when available), so closing your browser or restarting the server never kills your running agent.
> - **Single ~7.9 MB Rust Binary**: Powered by `Axum + Tokio`, zero Node/Electron runtime, ~33 MB RSS, embedded zero-build web UI.
>
> Built in Hengqin (HQ), built to be your coding agents' HQ. Would love feedback on the architecture and mobile workflow!
> Repo: https://github.com/beyondcy1013/hqcode

### Reddit (`r/ClaudeAI` / `r/rust` / `r/selfhosted` / `r/LocalLLaMA`)
**Title:**
`I built HQCode: a ~7.9 MB Rust control plane that keeps native Claude Code & Codex alive in tmux across desktop and phone (MIT)`

**Body:**
> Most coding-agent GUIs are heavy desktop wrappers (Electron/Tauri) that re-implement the terminal as chat cards—which breaks as soon as an upstream CLI adds a new interactive TUI feature, or freezes when you step away and try to check progress on your phone.
>
> **HQCode** keeps your agents 100% native inside isolated `tmux` sessions behind a single Rust binary:
> - **Survives disconnects & restarts**: Close your browser, switch from desktop to mobile, pick up the exact same live PTY backlog.
> - **Zero injection**: Never hijacks `CODEX_HOME` or `CLAUDE_CONFIG_DIR`.
> - **Lightweight Git Review right next to the terminal**: Inspect line-numbered diffs (`+N / -M`), revert a single file, or commit from desktop or mobile.
> - **Mobile soft-key bar**: One-tap `Esc`, `Tab`, `^C`, `↑`, `↓`, `y`, `n`, `↵` positioned cleanly with `visualViewport`.
>
> Repo (MIT): https://github.com/beyondcy1013/hqcode

### X / Twitter Thread
1. Stop stuffing Claude Code & Codex into fragile Electron chat wrappers that drop your session the moment you leave your desk. Meet **HQCode** (MIT, ~7.9 MB Rust binary): native `tmux` persistence + mobile soft-key steering + built-in Git diff review. 🧵👇
2. Core principle: **Terminal I/O Only**. Zero env injection, zero chat-card lag. Your agents run natively inside isolated `tmux` sockets, so closing tabs or restarting the server never kills your task.
3. On your phone? `visualViewport`-aware soft-key bar gives you one-tap `Esc`, `^C`, `y`, `n`, `↵` + slide-over Git Diff review + one-click commit. Built in Hengqin (HQ) to be your Agents' HQ → https://github.com/beyondcy1013/hqcode

### 中文社区（V2EX / Linux.do / 掘金 / 即刻）
**标题：**
`开源一个 7.9MB 的 Rust 单二进制项目 HQCode：用原生 tmux 常驻接管 Claude Code / Codex，手机与电脑无缝不断线`

**正文：**
> 最近像 T3 Code、Opcode 这类本地桌面端 Agent 外壳很火，但实际重度跑长任务时有两个痛点：一是离开工位想用手机看一眼进度或按个确认键非常别扭，二是桌面桥接一断或者 CLI 升级交互，外层套壳就容易卡住或滞后。
>
> 所以我把自用控制面里最核心的终端引擎抽出来，做成了一个纯 **MIT 开源**的单二进制工具 **HQCode**（Rust + Axum + `tmux` + `xterm.js`）：
> - **永不断线**：每个会话跑在隔离的 `tmux` socket 中，关掉浏览器、切到手机、甚至重启服务端，正在跑的 Claude Code / Codex 一字不丢；
> - **零侵入（Terminal I/O Only）**：不魔改、不注入 `CODEX_HOME` / `CLAUDE_CONFIG_DIR`，原生 CLI 能力 100% 保留；
> - **轻量 Git Diff 审阅抽屉**：终端旁直接看行级彩色 Diff、`+N / -M` 统计、单文件二次确认还原与一键提交；
> - **手机真正能用**：底部 `Esc` / `Tab` / `^C` / `y` / `n` / `↵` 软键盘栏配合 `visualViewport`，软键盘弹出不遮挡终端。
>
> 产物单文件仅约 7.9 MB，空载内存约 33 MB，支持中英文一键切换。欢迎 Star 体验与提 Issue！
