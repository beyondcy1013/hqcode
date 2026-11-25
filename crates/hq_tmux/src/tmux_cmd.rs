use std::{
    io::Read,
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use hq_runtime_paths::UserProfile;
use tracing::warn;

use crate::input_pacing::{ERASE_KEY_WRITE_GAP, erase_key_segments};

const TMUX_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const TMUX_SMALL_OUTPUT_LIMIT: usize = 64 * 1024;
const TMUX_SNAPSHOT_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const TMUX_HISTORY_LIMIT: &str = "100000";
const TMUX_TERMINAL_OVERRIDES: &str = "xterm-256color:indn@:rin@";
const CURSOR_HEADER_PREFIX: &str = "HQ_TMUX_CURSOR";

static TMUX_SCOPE_ISOLATION_AVAILABLE: AtomicBool = AtomicBool::new(true);

pub const CHILD_ENV_KEYS_TO_CLEAR: &[&str] = &[
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
    "WEBCLX_USER_HOME",
    "HQCODE_USER_HOME",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "TMUX",
    "TMUX_PANE",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmuxNamespace {
    pub socket_prefix: String,
    pub session_prefix: String,
    pub scope_unit_prefix: String,
    pub after_service: Option<String>,
}

impl TmuxNamespace {
    pub fn new(prefix: &str) -> Self {
        Self {
            socket_prefix: prefix.to_string(),
            session_prefix: prefix.to_string(),
            scope_unit_prefix: format!("{prefix}-tmux"),
            after_service: Some(format!("{prefix}.service")),
        }
    }

    pub fn socket_name(&self, session_id: &str) -> String {
        format!("{}_{}", self.socket_prefix, session_id)
    }

    pub fn session_name(&self, session_id: &str) -> String {
        format!("{}_{}", self.session_prefix, session_id)
    }

    pub fn scope_unit_name(&self, session_id: &str) -> String {
        format!("{}-{}", self.scope_unit_prefix, session_id)
    }
}

pub fn sanitize_child_command(command: &mut Command) {
    for key in CHILD_ENV_KEYS_TO_CLEAR {
        command.env_remove(key);
    }
}

pub fn sanitize_pty_command(command: &mut portable_pty::CommandBuilder) {
    for key in CHILD_ENV_KEYS_TO_CLEAR {
        command.env_remove(key);
    }
}

pub fn isolated_tmux_command(ns: &TmuxNamespace, session_id: &str) -> Command {
    let mut command = Command::new("tmux");
    command.arg("-L").arg(ns.socket_name(session_id));
    sanitize_child_command(&mut command);
    command
}

struct ChildKillGuard(Option<Child>);

impl ChildKillGuard {
    fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn take(&mut self) -> Option<Child> {
        self.0.take()
    }
}

impl Drop for ChildKillGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn read_pipe_bounded(mut reader: impl Read, max_bytes: usize) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if buf.len().saturating_add(n) > max_bytes {
            bail!("command output exceeded limit of {max_bytes} bytes");
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

pub fn bounded_command_output(
    mut command: Command,
    budget: Duration,
    max_bytes: usize,
) -> Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().context("failed to spawn command")?;
    let stdout = child.stdout.take().context("missing child stdout")?;
    let stderr = child.stderr.take().context("missing child stderr")?;
    let mut guard = ChildKillGuard::new(child);

    let out_handle = thread::spawn(move || read_pipe_bounded(stdout, max_bytes));
    let err_handle = thread::spawn(move || read_pipe_bounded(stderr, max_bytes));

    let deadline = Instant::now() + budget;
    let status = loop {
        let child_ref = guard.0.as_mut().context("missing child")?;
        if let Some(status) = child_ref.try_wait().context("waitpid failed")? {
            break status;
        }
        if Instant::now() >= deadline {
            drop(guard);
            let _ = out_handle.join();
            let _ = err_handle.join();
            bail!("command timed out after {}ms", budget.as_millis());
        }
        thread::sleep(Duration::from_millis(10));
    };

    let _finished_child = guard.take();
    let stdout_bytes = out_handle
        .join()
        .map_err(|_| anyhow::anyhow!("stdout reader panicked"))??;
    let stderr_bytes = err_handle
        .join()
        .map_err(|_| anyhow::anyhow!("stderr reader panicked"))??;

    if stdout_bytes.len().saturating_add(stderr_bytes.len()) > max_bytes {
        bail!("combined command output exceeded limit of {max_bytes} bytes");
    }

    Ok(Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    })
}

pub fn tmux_missing_session_error(stderr: &str) -> bool {
    let normalized = stderr.to_ascii_lowercase();
    normalized.contains("can't find session")
        || normalized.contains("no server running")
        || (normalized.trim().starts_with("error connecting to ")
            && normalized.trim().ends_with("(no such file or directory)"))
}

pub fn session_exists(ns: &TmuxNamespace, session_id: &str) -> bool {
    let mut cmd = isolated_tmux_command(ns, session_id);
    cmd.arg("has-session").arg("-t").arg(ns.session_name(session_id));
    match bounded_command_output(cmd, TMUX_COMMAND_TIMEOUT, TMUX_SMALL_OUTPUT_LIMIT) {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

fn should_isolate_tmux_scope() -> bool {
    #[cfg(test)]
    {
        return false;
    }
    #[allow(unreachable_code)]
    {
        TMUX_SCOPE_ISOLATION_AVAILABLE.load(Ordering::Relaxed)
            && std::env::var_os("INVOCATION_ID").is_some()
    }
}

fn should_disable_tmux_scope_isolation(error_message: &str) -> bool {
    let normalized = error_message.to_ascii_lowercase();
    normalized.contains("failed to start transient scope unit")
        || normalized.contains("interactive authentication required")
        || normalized.contains("authentication is required")
        || normalized.contains("access denied")
        || normalized.contains("failed to connect to bus")
        || normalized.contains("failed to create bus connection")
        || normalized.contains("transport endpoint is not connected")
        || normalized.contains("server exited unexpectedly")
}

/// Appends `runuser -u USER -- SHELL -l` (never `--login`, which breaks
/// isolated tmux servers on hosts rejecting combined `runuser -u` + `--login`).
pub fn append_runuser_login_shell(command: &mut Command, user_profile: &UserProfile) {
    command
        .arg("runuser")
        .arg("-u")
        .arg(&user_profile.name)
        .arg("--")
        .arg(&user_profile.shell)
        .arg("-l");
}

pub fn ensure_session(
    ns: &TmuxNamespace,
    session_id: &str,
    cwd: &Path,
    user_profile: &UserProfile,
) -> Result<()> {
    if session_exists(ns, session_id) {
        configure_tmux_server(ns, session_id)?;
        configure_tmux_session(ns, session_id)?;
        return Ok(());
    }

    if should_isolate_tmux_scope() {
        match create_tmux_session_in_scope(ns, session_id, cwd, user_profile) {
            Ok(()) => {
                configure_tmux_server(ns, session_id)?;
                configure_tmux_session(ns, session_id)?;
                return Ok(());
            }
            Err(error) => {
                let message = error.to_string();
                if should_disable_tmux_scope_isolation(&message) {
                    TMUX_SCOPE_ISOLATION_AVAILABLE.store(false, Ordering::Relaxed);
                }
                warn!(
                    "tmux scope creation failed for {}: {}; falling back to direct tmux launch",
                    session_id, message
                );
            }
        }
    }

    let cmd = isolated_tmux_command(ns, session_id);
    run_tmux_new_session(ns, cmd, session_id, cwd, user_profile)?;
    configure_tmux_server(ns, session_id)?;
    configure_tmux_session(ns, session_id)?;
    Ok(())
}

fn create_tmux_session_in_scope(
    ns: &TmuxNamespace,
    session_id: &str,
    cwd: &Path,
    user_profile: &UserProfile,
) -> Result<()> {
    let mut command = Command::new("systemd-run");
    sanitize_child_command(&mut command);
    command.arg("--scope").arg("--quiet");
    if let Some(after) = &ns.after_service {
        command.arg(format!("--property=After={after}"));
    }
    command
        .arg("--unit")
        .arg(ns.scope_unit_name(session_id))
        .arg("tmux")
        .arg("-L")
        .arg(ns.socket_name(session_id));
    run_tmux_new_session(ns, command, session_id, cwd, user_profile)
}

fn run_tmux_new_session(
    ns: &TmuxNamespace,
    mut command: Command,
    session_id: &str,
    cwd: &Path,
    user_profile: &UserProfile,
) -> Result<()> {
    let build_tmux_args = |cmd: &mut Command| {
        cmd.arg("new-session")
            .arg("-d")
            .arg("-s")
            .arg(ns.session_name(session_id))
            .arg("-c")
            .arg(cwd);
    };

    build_tmux_args(&mut command);
    #[cfg(unix)]
    let can_runuser = unsafe { libc::geteuid() == 0 };
    #[cfg(not(unix))]
    let can_runuser = false;

    if can_runuser {
        append_runuser_login_shell(&mut command, user_profile);
    } else {
        command.arg(&user_profile.shell).arg("-l");
    }

    let output = bounded_command_output(command, TMUX_COMMAND_TIMEOUT, TMUX_SMALL_OUTPUT_LIMIT)
        .context("failed to create tmux session")?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if can_runuser
        && (stderr.to_ascii_lowercase().contains("runuser")
            || stderr.to_ascii_lowercase().contains("not found"))
    {
        let mut fallback = isolated_tmux_command(ns, session_id);
        build_tmux_args(&mut fallback);
        fallback.arg(&user_profile.shell).arg("-l");
        let retry = bounded_command_output(fallback, TMUX_COMMAND_TIMEOUT, TMUX_SMALL_OUTPUT_LIMIT)?;
        if retry.status.success() {
            return Ok(());
        }
        let retry_err = String::from_utf8_lossy(&retry.stderr).trim().to_string();
        bail!("failed to create tmux session (fallback): {retry_err}");
    }

    bail!("failed to create tmux session: {stderr}")
}

fn configure_tmux_server(ns: &TmuxNamespace, session_id: &str) -> Result<()> {
    let mut cmd = isolated_tmux_command(ns, session_id);
    cmd.args(["set-option", "-sg", "escape-time", "0"]);
    let output = bounded_command_output(cmd, TMUX_COMMAND_TIMEOUT, TMUX_SMALL_OUTPUT_LIMIT)?;
    if !output.status.success() {
        bail!(
            "failed to configure tmux escape-time: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn configure_tmux_session(ns: &TmuxNamespace, session_id: &str) -> Result<()> {
    for (option, value) in [
        ("history-limit", TMUX_HISTORY_LIMIT),
        ("status", "off"),
        ("terminal-overrides", TMUX_TERMINAL_OVERRIDES),
        ("focus-events", "on"),
    ] {
        let mut cmd = isolated_tmux_command(ns, session_id);
        cmd.arg("set-option")
            .arg("-t")
            .arg(ns.session_name(session_id))
            .arg(option)
            .arg(value);
        let output = bounded_command_output(cmd, TMUX_COMMAND_TIMEOUT, TMUX_SMALL_OUTPUT_LIMIT)?;
        if !output.status.success() {
            bail!(
                "failed to set tmux option {option}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    Ok(())
}

pub fn destroy_session(ns: &TmuxNamespace, session_id: &str) -> Result<()> {
    let mut cmd = isolated_tmux_command(ns, session_id);
    cmd.args(["kill-session", "-t", &ns.session_name(session_id)]);
    let output = bounded_command_output(cmd, TMUX_COMMAND_TIMEOUT, TMUX_SMALL_OUTPUT_LIMIT)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() && !tmux_missing_session_error(&stderr) {
        bail!("failed to kill tmux session: {}", stderr.trim());
    }
    let mut kill_srv = isolated_tmux_command(ns, session_id);
    kill_srv.arg("kill-server");
    let _ = bounded_command_output(kill_srv, TMUX_COMMAND_TIMEOUT, TMUX_SMALL_OUTPUT_LIMIT);
    Ok(())
}

pub fn send_input(ns: &TmuxNamespace, session_id: &str, data: &str) -> Result<()> {
    let (text, submit_count) = split_submit_keys(data);
    for (index, segment) in erase_key_segments(text).into_iter().enumerate() {
        if index > 0 {
            thread::sleep(ERASE_KEY_WRITE_GAP);
        }
        send_literal_keys(ns, session_id, segment)?;
    }
    for _ in 0..submit_count {
        send_enter(ns, session_id)?;
    }
    Ok(())
}

pub fn send_enter(ns: &TmuxNamespace, session_id: &str) -> Result<()> {
    let mut cmd = isolated_tmux_command(ns, session_id);
    cmd.args(["send-keys", "-t", &ns.session_name(session_id), "C-m"]);
    let out = bounded_command_output(cmd, TMUX_COMMAND_TIMEOUT, TMUX_SMALL_OUTPUT_LIMIT)?;
    if !out.status.success() {
        bail!(
            "failed to send Enter: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn send_literal_keys(ns: &TmuxNamespace, session_id: &str, text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let mut cmd = isolated_tmux_command(ns, session_id);
    cmd.args(["send-keys", "-t", &ns.session_name(session_id), "-l", text]);
    let out = bounded_command_output(cmd, TMUX_COMMAND_TIMEOUT, TMUX_SMALL_OUTPUT_LIMIT)?;
    if !out.status.success() {
        bail!(
            "failed to send keys: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn split_submit_keys(data: &str) -> (&str, usize) {
    let mut text_end = data.len();
    let mut submit_count = 0;
    while text_end > 0 {
        let Some((index, ch)) = data[..text_end].char_indices().next_back() else {
            break;
        };
        if ch != '\r' && ch != '\n' {
            break;
        }
        text_end = index;
        submit_count += 1;
    }
    (&data[..text_end], submit_count)
}

pub fn capture_initial_snapshot(
    ns: &TmuxNamespace,
    session_id: &str,
    line_limit: u32,
) -> Result<Vec<u8>> {
    let target = ns.session_name(session_id);
    let mut cmd = isolated_tmux_command(ns, session_id);
    cmd.arg("display-message")
        .arg("-p")
        .arg("-t")
        .arg(&target)
        .arg(format!(
            "{CURSOR_HEADER_PREFIX} #{{cursor_x}} #{{cursor_y}} #{{cursor_flag}}"
        ))
        .arg(";")
        .arg("capture-pane")
        .arg("-e")
        .arg("-p")
        .arg("-S")
        .arg(format!("-{line_limit}"))
        .arg("-t")
        .arg(&target);
    let out = bounded_command_output(cmd, TMUX_COMMAND_TIMEOUT, TMUX_SNAPSHOT_OUTPUT_LIMIT)?;
    if !out.status.success() {
        bail!(
            "failed to capture tmux snapshot: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let (cursor, body) = split_cursor_header(&out.stdout);
    Ok(normalize_replay_snapshot(body, cursor))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CursorState {
    x: u16,
    y: u16,
    visible: bool,
}

fn split_cursor_header(bytes: &[u8]) -> (Option<CursorState>, &[u8]) {
    let Some(nl) = bytes.iter().position(|b| *b == b'\n') else {
        return (None, bytes);
    };
    let header = bytes[..nl].strip_suffix(b"\r").unwrap_or(&bytes[..nl]);
    let Ok(text) = std::str::from_utf8(header) else {
        return (None, bytes);
    };
    let mut parts = text.split_ascii_whitespace();
    let (Some(prefix), Some(x), Some(y), Some(flag), None) = (
        parts.next(),
        parts.next().and_then(|v| v.parse::<u16>().ok()),
        parts.next().and_then(|v| v.parse::<u16>().ok()),
        parts.next().and_then(|v| v.parse::<u8>().ok()),
        parts.next(),
    ) else {
        return (None, bytes);
    };
    if prefix != CURSOR_HEADER_PREFIX {
        return (None, bytes);
    }
    (
        Some(CursorState {
            x,
            y,
            visible: flag != 0,
        }),
        &bytes[nl + 1..],
    )
}

fn normalize_replay_snapshot(bytes: &[u8], cursor: Option<CursorState>) -> Vec<u8> {
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if body.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&[u8]> = body.split(|b| *b == b'\n').collect();
    let mut normalized = Vec::with_capacity(body.len() + lines.len() * 2 + 32);
    for (idx, line) in lines.iter().enumerate() {
        normalized.extend_from_slice(line.strip_suffix(b"\r").unwrap_or(line));
        if idx + 1 < lines.len() {
            normalized.extend_from_slice(b"\r\n");
        }
    }
    normalized.extend_from_slice(b"\x1b[0m");
    if let Some(c) = cursor {
        let row = u32::from(c.y) + 1;
        let col = u32::from(c.x) + 1;
        let vis = if c.visible { "\x1b[?25h" } else { "\x1b[?25l" };
        normalized.extend_from_slice(format!("\x1b[{row};{col}H{vis}").as_bytes());
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TmuxGuard {
        ns: TmuxNamespace,
        session_id: String,
    }

    impl Drop for TmuxGuard {
        fn drop(&mut self) {
            let _ = destroy_session(&self.ns, &self.session_id);
        }
    }

    #[test]
    fn namespace_isolates_sockets_and_sessions() {
        let hq = TmuxNamespace::new("hqcode");
        let wc = TmuxNamespace::new("webclx");
        assert_eq!(hq.socket_name("s1"), "hqcode_s1");
        assert_eq!(wc.socket_name("s1"), "webclx_s1");
        assert_ne!(hq.socket_name("s1"), wc.socket_name("s1"));
    }

    #[test]
    fn runuser_args_never_contain_double_dash_login() {
        let profile = hq_runtime_paths::resolve_current_user().unwrap();
        let mut cmd = Command::new("tmux");
        append_runuser_login_shell(&mut cmd, &profile);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "runuser",
                "-u",
                &profile.name,
                "--",
                &profile.shell.to_string_lossy(),
                "-l"
            ]
        );
        assert!(!args.iter().any(|a| a == "--login"));
    }

    #[test]
    fn live_tmux_session_create_input_snapshot_and_cleanup() {
        if Command::new("tmux").arg("-V").output().is_err() {
            return;
        }
        let ns = TmuxNamespace::new("hqcode-test");
        let sid = format!("u{}", std::process::id());
        let _guard = TmuxGuard {
            ns: ns.clone(),
            session_id: sid.clone(),
        };
        let profile = hq_runtime_paths::resolve_current_user().unwrap();
        ensure_session(&ns, &sid, Path::new("/tmp"), &profile).unwrap();
        assert!(session_exists(&ns, &sid));

        send_input(&ns, &sid, "echo HQCODE_TMUX_OK\r").unwrap();
        thread::sleep(Duration::from_millis(200));
        let snap = capture_initial_snapshot(&ns, &sid, 200).unwrap();
        assert!(String::from_utf8_lossy(&snap).contains("HQCODE_TMUX_OK"));

        destroy_session(&ns, &sid).unwrap();
        assert!(!session_exists(&ns, &sid));
    }
}
