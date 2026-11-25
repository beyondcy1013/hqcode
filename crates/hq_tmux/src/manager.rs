use std::{
    collections::{HashMap, VecDeque},
    io::{ErrorKind, Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use hq_runtime_paths::UserProfile;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::tmux_cmd::{
    TmuxNamespace, capture_initial_snapshot, destroy_session, ensure_session, sanitize_pty_command,
    send_enter as tmux_send_enter, send_input as tmux_send_input, session_exists,
};

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 34;
const MAX_BACKLOG_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_SNAPSHOT_LINES: u32 = 5000;
const TERMINAL_OUTPUT_CHANNEL_CAPACITY: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalViewportSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug)]
pub struct TerminalOutputChunk {
    pub seq: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum TerminalStreamEvent {
    Output(TerminalOutputChunk),
    ViewportSize(TerminalViewportSize),
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: String,
    pub name: String,
    pub cwd: PathBuf,
    pub created_at: u64,
    pub last_used_at: u64,
    pub manually_renamed: bool,
    pub user_name: String,
    pub alive: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub cwd: Option<String>,
    pub name: Option<String>,
    pub startup_command: Option<String>,
}

pub struct SessionSnapshot {
    pub replay_bytes: Vec<u8>,
    pub replay_end_seq: u64,
    pub alive: bool,
    pub receiver: broadcast::Receiver<TerminalStreamEvent>,
}

#[derive(Debug, Default)]
struct OutputBacklog {
    chunks: VecDeque<TerminalOutputChunk>,
    total_bytes: usize,
}

impl OutputBacklog {
    fn seed(&mut self, bytes: &[u8]) {
        self.chunks.clear();
        self.total_bytes = 0;
        if bytes.is_empty() {
            return;
        }
        let slice = if bytes.len() > MAX_BACKLOG_BYTES {
            &bytes[bytes.len() - MAX_BACKLOG_BYTES..]
        } else {
            bytes
        };
        self.total_bytes = slice.len();
        self.chunks.push_back(TerminalOutputChunk {
            seq: 0,
            bytes: slice.to_vec(),
        });
    }

    fn push(&mut self, chunk: TerminalOutputChunk) {
        self.total_bytes = self.total_bytes.saturating_add(chunk.bytes.len());
        self.chunks.push_back(chunk);
        while self.total_bytes > MAX_BACKLOG_BYTES {
            let Some(front) = self.chunks.pop_front() else {
                self.total_bytes = 0;
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(front.bytes.len());
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.total_bytes);
        for chunk in &self.chunks {
            out.extend_from_slice(&chunk.bytes);
        }
        out
    }
}

#[derive(Debug, Default)]
struct ViewportRegistry {
    next_id: u64,
    clients: HashMap<u64, TerminalViewportSize>,
}

impl ViewportRegistry {
    fn register(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }

    fn update(&mut self, client_id: u64, cols: u16, rows: u16) -> Option<TerminalViewportSize> {
        let size = TerminalViewportSize {
            cols: cols.clamp(20, 400),
            rows: rows.clamp(6, 160),
        };
        self.clients.insert(client_id, size);
        self.clients.values().max_by_key(|s| (s.cols, s.rows)).copied()
    }

    fn unregister(&mut self, client_id: u64) {
        self.clients.remove(&client_id);
    }
}

struct LiveSession {
    meta: RwLock<SessionMetadata>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    viewports: Mutex<ViewportRegistry>,
    backlog: Mutex<OutputBacklog>,
    next_seq: AtomicU64,
    broadcaster: broadcast::Sender<TerminalStreamEvent>,
    alive: AtomicBool,
    expected_shutdown: AtomicBool,
    attached_at: Instant,
    suppress_redraw: AtomicBool,
}

impl LiveSession {
    fn attach(
        ns: &TmuxNamespace,
        meta: SessionMetadata,
        initial_snapshot: Option<Vec<u8>>,
    ) -> Result<Arc<Self>> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open PTY")?;

        let cwd = if meta.cwd.is_dir() {
            meta.cwd.clone()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
        };

        let mut cmd = CommandBuilder::new("tmux");
        cmd.args([
            "-L",
            &ns.socket_name(&meta.id),
            "attach-session",
            "-t",
            &ns.session_name(&meta.id),
        ]);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        sanitize_pty_command(&mut cmd);

        let child = pair
            .slave
            .spawn_command(cmd)
            .context("failed to attach tmux session")?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let writer = pair.master.take_writer().context("failed to take PTY writer")?;

        let has_seed = initial_snapshot.is_some();
        let (broadcaster, _) = broadcast::channel(TERMINAL_OUTPUT_CHANNEL_CAPACITY);
        let session = Arc::new(Self {
            meta: RwLock::new(meta),
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            viewports: Mutex::new(ViewportRegistry::default()),
            backlog: Mutex::new(OutputBacklog::default()),
            next_seq: AtomicU64::new(0),
            broadcaster,
            alive: AtomicBool::new(true),
            expected_shutdown: AtomicBool::new(false),
            attached_at: Instant::now(),
            suppress_redraw: AtomicBool::new(has_seed),
        });

        if let Some(seed) = initial_snapshot {
            session
                .backlog
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .seed(&seed);
        }

        Self::spawn_reader(session.clone(), reader);
        Ok(session)
    }

    fn spawn_reader(session: Arc<Self>, mut reader: Box<dyn Read + Send>) {
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        session.mark_closed(b"");
                        break;
                    }
                    Ok(n) => {
                        if session.expected_shutdown.load(Ordering::SeqCst) {
                            continue;
                        }
                        if session.suppress_redraw.load(Ordering::Relaxed) {
                            if session.attached_at.elapsed().as_millis() < 250 {
                                continue;
                            }
                            session.suppress_redraw.store(false, Ordering::Relaxed);
                        }
                        let seq = session.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
                        let chunk = TerminalOutputChunk {
                            seq,
                            bytes: buffer[..n].to_vec(),
                        };
                        {
                            let mut backlog =
                                session.backlog.lock().unwrap_or_else(|p| p.into_inner());
                            backlog.push(chunk.clone());
                        }
                        let _ = session.broadcaster.send(TerminalStreamEvent::Output(chunk));
                    }
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) => {
                        let msg = format!("\r\n[terminal] read error: {e}\r\n");
                        session.mark_closed(msg.as_bytes());
                        break;
                    }
                }
            }
        });
    }

    fn mark_closed(&self, trailing: &[u8]) {
        if !trailing.is_empty() {
            let seq = self.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
            let chunk = TerminalOutputChunk {
                seq,
                bytes: trailing.to_vec(),
            };
            self.backlog
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(chunk.clone());
            let _ = self.broadcaster.send(TerminalStreamEvent::Output(chunk));
        }
        self.alive.store(false, Ordering::SeqCst);
        let _ = self.broadcaster.send(TerminalStreamEvent::Closed);
    }

    fn write_pty(&self, bytes: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    fn register_client(&self) -> u64 {
        self.viewports
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .register()
    }

    fn unregister_client(&self, client_id: u64) {
        self.viewports
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .unregister(client_id);
    }

    fn resize_client(&self, client_id: u64, cols: u16, rows: u16) -> Result<()> {
        let effective = self
            .viewports
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .update(client_id, cols, rows);
        if let Some(size) = effective {
            self.master
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .resize(PtySize {
                    rows: size.rows,
                    cols: size.cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })?;
            let _ = self.broadcaster.send(TerminalStreamEvent::ViewportSize(size));
        }
        Ok(())
    }

    fn snapshot_and_subscribe(&self) -> SessionSnapshot {
        let backlog = self.backlog.lock().unwrap_or_else(|p| p.into_inner());
        let replay_bytes = backlog.snapshot();
        let replay_end_seq = self.next_seq.load(Ordering::SeqCst);
        let receiver = self.broadcaster.subscribe();
        SessionSnapshot {
            replay_bytes,
            replay_end_seq,
            alive: self.alive.load(Ordering::SeqCst),
            receiver,
        }
    }

    fn terminate_pty_client(&self) {
        self.expected_shutdown.store(true, Ordering::SeqCst);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedState {
    sessions: Vec<SessionMetadata>,
}

#[derive(Clone)]
pub struct TerminalManager {
    ns: TmuxNamespace,
    state_file: PathBuf,
    default_cwd: PathBuf,
    user_profile: UserProfile,
    sessions: Arc<RwLock<HashMap<String, Arc<LiveSession>>>>,
    stored_order: Arc<RwLock<Vec<SessionMetadata>>>,
}

impl TerminalManager {
    pub fn new(
        ns: TmuxNamespace,
        state_file: PathBuf,
        default_cwd: PathBuf,
        user_profile: UserProfile,
    ) -> Self {
        let stored = hq_fs_atomic::read_private_json::<PersistedState>(&state_file)
            .map(|s| s.sessions)
            .unwrap_or_default();

        let mgr = Self {
            ns,
            state_file,
            default_cwd,
            user_profile,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            stored_order: Arc::new(RwLock::new(stored)),
        };
        mgr.restore_live_sessions();
        mgr
    }

    fn restore_live_sessions(&self) {
        let stored = self
            .stored_order
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut live_list = Vec::new();
        for mut meta in stored {
            if session_exists(&self.ns, &meta.id) {
                let snapshot = capture_initial_snapshot(&self.ns, &meta.id, DEFAULT_SNAPSHOT_LINES).ok();
                match LiveSession::attach(&self.ns, meta.clone(), snapshot) {
                    Ok(live) => {
                        meta.alive = true;
                        self.sessions
                            .write()
                            .unwrap_or_else(|p| p.into_inner())
                            .insert(meta.id.clone(), live);
                        live_list.push(meta);
                    }
                    Err(err) => {
                        warn!("failed to reattach tmux session {}: {err}", meta.id);
                    }
                }
            }
        }
        *self.stored_order.write().unwrap_or_else(|p| p.into_inner()) = live_list;
        self.persist_state();
    }

    fn persist_state(&self) {
        let list = self.list_sessions();
        let state = PersistedState { sessions: list };
        if let Err(err) = hq_fs_atomic::write_private_json(&self.state_file, &state) {
            debug!("failed to persist terminal state: {err}");
        }
    }

    pub fn list_sessions(&self) -> Vec<SessionMetadata> {
        let stored = self
            .stored_order
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let live_map = self.sessions.read().unwrap_or_else(|p| p.into_inner());
        let mut out = Vec::with_capacity(stored.len());
        for mut meta in stored {
            if let Some(live) = live_map.get(&meta.id) {
                let current = live.meta.read().unwrap_or_else(|p| p.into_inner()).clone();
                meta = current;
                meta.alive = live.alive.load(Ordering::SeqCst);
            } else {
                meta.alive = false;
            }
            out.push(meta);
        }
        out.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));
        out
    }

    pub fn create_session(&self, req: CreateSessionRequest) -> Result<SessionMetadata> {
        let cwd = req
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_cwd.clone());
        if !cwd.is_dir() {
            bail!("workspace directory does not exist: {}", cwd.display());
        }

        let id = format!(
            "s{:x}{:04x}",
            now_unix_secs(),
            rand::random::<u16>()
        );
        let dir_label = cwd
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("shell");
        let existing_count = self.list_sessions().len() + 1;
        let name = req
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{dir_label}-{existing_count}"));

        let now = now_unix_secs();
        let meta = SessionMetadata {
            id: id.clone(),
            name,
            cwd: cwd.clone(),
            created_at: now,
            last_used_at: now,
            manually_renamed: req.name.is_some(),
            user_name: self.user_profile.name.clone(),
            alive: true,
        };

        ensure_session(&self.ns, &id, &cwd, &self.user_profile)?;
        if let Some(cmd) = req
            .startup_command
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            tmux_send_input(&self.ns, &id, &format!("{cmd}\r"))?;
        }

        let snapshot = capture_initial_snapshot(&self.ns, &id, DEFAULT_SNAPSHOT_LINES).ok();
        let live = LiveSession::attach(&self.ns, meta.clone(), snapshot)?;
        self.sessions
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.clone(), live);
        self.stored_order
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .push(meta.clone());
        self.persist_state();
        Ok(meta)
    }

    pub fn rename_session(&self, session_id: &str, new_name: &str) -> Result<SessionMetadata> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            bail!("session name cannot be empty");
        }
        let live = self.get_live(session_id)?;
        let updated = {
            let mut guard = live.meta.write().unwrap_or_else(|p| p.into_inner());
            guard.name = trimmed.to_string();
            guard.manually_renamed = true;
            guard.last_used_at = now_unix_secs();
            guard.clone()
        };
        {
            let mut stored = self.stored_order.write().unwrap_or_else(|p| p.into_inner());
            if let Some(item) = stored.iter_mut().find(|m| m.id == session_id) {
                *item = updated.clone();
            }
        }
        self.persist_state();
        Ok(updated)
    }

    pub fn close_session(&self, session_id: &str) -> Result<()> {
        let live = self
            .sessions
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(session_id);
        if let Some(live) = live {
            live.terminate_pty_client();
        }
        destroy_session(&self.ns, session_id)?;
        self.stored_order
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|m| m.id != session_id);
        self.persist_state();
        Ok(())
    }

    pub fn write_raw_input(&self, session_id: &str, data: &str) -> Result<()> {
        let live = self.get_live(session_id)?;
        live.write_pty(data.as_bytes())?;
        Ok(())
    }

    pub fn send_input(&self, session_id: &str, data: &str) -> Result<()> {
        let _ = self.get_live(session_id)?;
        tmux_send_input(&self.ns, session_id, data)
    }

    pub fn send_enter(&self, session_id: &str) -> Result<()> {
        let _ = self.get_live(session_id)?;
        tmux_send_enter(&self.ns, session_id)
    }

    pub fn subscribe(&self, session_id: &str) -> Result<(u64, SessionSnapshot)> {
        let live = self.get_live(session_id)?;
        let client_id = live.register_client();
        let snap = live.snapshot_and_subscribe();
        Ok((client_id, snap))
    }

    pub fn unregister_client(&self, session_id: &str, client_id: u64) {
        if let Ok(live) = self.get_live(session_id) {
            live.unregister_client(client_id);
        }
    }

    pub fn resize_client(&self, session_id: &str, client_id: u64, cols: u16, rows: u16) -> Result<()> {
        let live = self.get_live(session_id)?;
        live.resize_client(client_id, cols, rows)
    }

    fn get_live(&self, session_id: &str) -> Result<Arc<LiveSession>> {
        self.sessions
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(session_id)
            .cloned()
            .with_context(|| format!("terminal session not found: {session_id}"))
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ManagerGuard {
        mgr: TerminalManager,
        dir: PathBuf,
        created_ids: Vec<String>,
    }

    impl Drop for ManagerGuard {
        fn drop(&mut self) {
            for id in &self.created_ids {
                let _ = self.mgr.close_session(id);
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn create_send_rename_and_close_session() {
        if std::process::Command::new("tmux").arg("-V").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("hq-mgr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let profile = hq_runtime_paths::resolve_current_user().unwrap();
        let mgr = TerminalManager::new(
            TmuxNamespace::new("hqcode-mgr-test"),
            dir.join("sessions.json"),
            PathBuf::from("/tmp"),
            profile,
        );
        let mut guard = ManagerGuard {
            mgr: mgr.clone(),
            dir,
            created_ids: Vec::new(),
        };

        let meta = mgr
            .create_session(CreateSessionRequest {
                cwd: Some("/tmp".to_string()),
                name: None,
                startup_command: Some("echo MANAGER_OK".to_string()),
            })
            .unwrap();
        guard.created_ids.push(meta.id.clone());

        let renamed = mgr.rename_session(&meta.id, "dev-main").unwrap();
        assert_eq!(renamed.name, "dev-main");

        let (cid, snap) = mgr.subscribe(&meta.id).unwrap();
        assert!(snap.alive);
        mgr.unregister_client(&meta.id, cid);

        mgr.close_session(&meta.id).unwrap();
        guard.created_ids.clear();
        assert!(mgr.list_sessions().is_empty());
    }
}
