use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Instant,
};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::{
        ConnectInfo, FromRequestParts, Path as AxumPath, Query, Request, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post, put},
};
use futures_util::{SinkExt, StreamExt};
use hq_auth::{AuthConfig, AuthService};
use hq_presets::{
    DetectedAgent, ModelPreset, PresetKind, PresetStore, apply_preset, detect_installed_agents,
    import_current_config, inspect_active_configs,
};
use hq_remote_connect::{
    EndpointKind, RemoteConnectConfig, RemoteConnectService, discover_endpoints,
    preferred_phone_endpoint,
};
use hq_runtime_paths::resolve_current_user;
use hq_tmux::{CreateSessionRequest, TerminalManager, TerminalStreamEvent, TmuxNamespace};
use hq_workspace::{
    WorkspaceConfig, git_commit, git_diff, git_revert_file, git_status, list_entries,
    read_utf8_file, rename_entry, resolve_directory_path, write_utf8_file,
};
use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tower_http::compression::CompressionLayer;
use tracing::{info, warn};

static EMBEDDED_STATIC_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/static");
const NO_STORE_CACHE: &str = "no-store, max-age=0, must-revalidate";
const STATIC_CACHE: &str = "no-cache, must-revalidate";

#[derive(Clone)]
pub struct AppState {
    pub version: String,
    pub started_at: Instant,
    pub static_dir: PathBuf,
    pub workspace: Arc<RwLock<WorkspaceConfig>>,
    pub auth: AuthService,
    pub terminal: TerminalManager,
    pub presets: PresetStore,
    pub user_home: PathBuf,
    pub bind_addr: SocketAddr,
    pub public_url: Option<String>,
    pub remote: RemoteConnectService,
}

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }

    fn unauthorized(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: msg.into(),
        }
    }

    fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }

    fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

type ApiResult<T> = std::result::Result<T, AppError>;

pub async fn serve(
    addr: SocketAddr,
    data_dir: PathBuf,
    workspace_dir: PathBuf,
    public_url: Option<String>,
    no_qr: bool,
) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hqcode=info,tower_http=info".into()),
        )
        .init();

    let canonical_workspace = workspace_dir
        .canonicalize()
        .with_context(|| format!("workspace does not exist: {}", workspace_dir.display()))?;
    let user_profile = resolve_current_user().context("failed to resolve current OS user")?;
    let user_home = hq_runtime_paths::resolve_user_home_preferring_env(&user_profile.name)
        .unwrap_or_else(|_| user_profile.home.clone());

    let auth = AuthService::initialize(&data_dir, AuthConfig::for_app("hqcode"))?;
    let remote =
        RemoteConnectService::initialize(&data_dir, RemoteConnectConfig::for_app("hqcode"))?;
    let terminal = TerminalManager::new(
        TmuxNamespace::new("hqcode"),
        data_dir.join("hqcode-sessions.json"),
        canonical_workspace.clone(),
        user_profile,
    );
    let presets = PresetStore::load(&data_dir, "hqcode");

    let static_dir = resolve_static_dir();
    let ws_config = WorkspaceConfig {
        display_root: workspace_dir.clone(),
        root: canonical_workspace.clone(),
        show_dot_entries: true,
        allow_one_parent: true,
    };

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    let actual_addr = listener.local_addr()?;

    let state = AppState {
        version: env!("CARGO_PKG_VERSION").to_string(),
        started_at: Instant::now(),
        static_dir,
        workspace: Arc::new(RwLock::new(ws_config)),
        auth: auth.clone(),
        terminal,
        presets,
        user_home,
        bind_addr: actual_addr,
        public_url: public_url.clone(),
        remote: remote.clone(),
    };

    info!(
        "HQCode v{} listening on http://{}",
        state.version, actual_addr
    );
    info!("workspace: {}", canonical_workspace.display());
    info!("data dir: {}", data_dir.display());
    print_startup_banner(
        &state.version,
        actual_addr,
        public_url.as_deref(),
        &canonical_workspace,
        &data_dir,
        &auth,
        &remote,
        no_qr,
    );

    let app = build_router(state);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .context("server exited with error")
}

#[allow(clippy::too_many_arguments)]
fn print_startup_banner(
    version: &str,
    bind_addr: SocketAddr,
    public_url: Option<&str>,
    workspace: &Path,
    data_dir: &Path,
    auth: &AuthService,
    remote: &RemoteConnectService,
    no_qr: bool,
) {
    let endpoints = discover_endpoints(bind_addr, public_url);
    println!("\n=== HQCode v{version} ========================================");
    println!("  Local WebUI : http://127.0.0.1:{}", bind_addr.port());
    for ep in &endpoints {
        if ep.kind != EndpointKind::Loopback {
            println!("  {:<12}: {}", ep.label, ep.url);
        }
    }
    println!("  Workspace   : {}", workspace.display());
    println!("  Data Dir    : {}", data_dir.display());
    if let Some((user, pw)) = auth.read_initial_password() {
        println!("  First Login : username=`{user}`  password=`{pw}`");
        println!("  Note        : initial password file is deleted after first login.");
    }
    if !no_qr {
        if let Some(phone_ep) = preferred_phone_endpoint(&endpoints) {
            match remote.issue_ticket(&phone_ep.url, Some("CLI Startup"), Some(600)) {
                Ok(issued) => {
                    println!(
                        "\n  Phone QR Pairing (one-time, valid 10m) -> {}:",
                        issued.ticket.pair_url
                    );
                    println!("{}", issued.qr_terminal);
                }
                Err(err) => {
                    warn!("failed to generate startup QR code: {err}");
                }
            }
        } else {
            println!(
                "  Phone QR    : loopback-only mode; restart with `--lan` or `--public-url <URL>` to scan from phone."
            );
        }
    }
    println!("============================================================\n");
}

fn resolve_static_dir() -> PathBuf {
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("static");
        if candidate.is_dir() {
            return candidate;
        }
    }
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/static"))
}

pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/auth/logout", post(api_logout))
        .route("/api/auth/password", post(api_change_password))
        .route("/api/system/info", get(api_system_info))
        .route("/api/agents/detected", get(api_detected_agents))
        .route("/api/entries", get(api_list_entries))
        .route("/api/file", get(api_read_file).put(api_save_file))
        .route("/api/file/rename", post(api_rename_path))
        .route("/api/git/status", get(api_git_status))
        .route("/api/git/diff", get(api_git_diff))
        .route("/api/git/revert", post(api_git_revert))
        .route("/api/git/commit", post(api_git_commit))
        .route(
            "/api/terminal/sessions",
            get(api_list_sessions).post(api_create_session),
        )
        .route("/api/terminal/sessions/search", get(api_search_sessions))
        .route(
            "/api/terminal/sessions/{id}",
            put(api_rename_session).delete(api_close_session),
        )
        .route("/api/terminal/sessions/{id}/input", post(api_send_input))
        .route("/api/terminal/sessions/{id}/enter", post(api_send_enter))
        .route("/ws/terminal/{id}", get(ws_terminal))
        .route(
            "/api/presets",
            get(api_list_presets).post(api_upsert_preset),
        )
        .route("/api/presets/active", get(api_active_presets))
        .route("/api/presets/import", post(api_import_preset))
        .route("/api/presets/{id}", delete(api_delete_preset))
        .route("/api/presets/{id}/apply", post(api_apply_preset))
        .route("/api/remote/status", get(api_remote_status))
        .route("/api/remote/tickets", post(api_remote_issue_ticket))
        .route("/api/remote/tickets/{id}", delete(api_remote_revoke_ticket))
        .route("/api/remote/devices/{id}", delete(api_remote_revoke_device))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .route("/", get(serve_index))
        .route("/login", get(serve_index))
        .route("/pair", get(serve_index))
        .route("/favicon.ico", get(serve_favicon))
        .route("/static/{*path}", get(serve_static))
        .route("/api/health", get(api_health))
        .route("/api/auth/session", get(api_auth_session))
        .route("/api/auth/login", post(api_login))
        .route("/api/remote/redeem", post(api_remote_redeem))
        .merge(protected)
        .fallback(get(serve_index))
        .layer(CompressionLayer::new())
        .with_state(state)
}

fn verify_cookie_is_active(state: &AppState, token: &str) -> bool {
    let Some(claims) = state.auth.verify_session_cookie_claims(token) else {
        return false;
    };
    if let Some(ref did) = claims.device_id {
        if !state.remote.is_device_active(did) {
            return false;
        }
        state.remote.touch_device(did);
    }
    true
}

async fn require_auth(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let is_loopback = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().is_loopback())
        .unwrap_or(false);

    if is_loopback
        && let Some(auth_hdr) = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
        && let Some(token) = auth_hdr.strip_prefix("Bearer ")
        && state.auth.verify_local_token(token.trim())
    {
        return next.run(request).await;
    }

    if let Some(cookie_hdr) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok())
        && let Some(token) = state.auth.extract_cookie_value(cookie_hdr)
        && verify_cookie_is_active(&state, token)
    {
        return next.run(request).await;
    }

    AppError::unauthorized("authentication required").into_response()
}

async fn serve_index(State(state): State<AppState>) -> Response {
    let disk_path = state.static_dir.join("index.html");
    let html = tokio::fs::read_to_string(&disk_path)
        .await
        .ok()
        .or_else(|| {
            EMBEDDED_STATIC_DIR
                .get_file("index.html")
                .and_then(|f| f.contents_utf8())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "<h1>HQCode</h1><p>Missing index.html</p>".to_string());

    let mut resp = Html(html).into_response();
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(NO_STORE_CACHE),
    );
    resp
}

async fn serve_favicon(State(state): State<AppState>, req_headers: HeaderMap) -> Response {
    serve_static_asset(&state, "favicon.ico", &req_headers).await
}

async fn serve_static(
    State(state): State<AppState>,
    AxumPath(asset_path): AxumPath<String>,
    req_headers: HeaderMap,
) -> Response {
    let clean = asset_path.trim_start_matches('/');
    serve_static_asset(&state, clean, &req_headers).await
}

async fn serve_static_asset(state: &AppState, clean: &str, req_headers: &HeaderMap) -> Response {
    if clean.is_empty() || clean.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }

    let disk_file = state.static_dir.join(clean);
    let bytes = if let Ok(data) = tokio::fs::read(&disk_file).await {
        data
    } else if let Some(embedded) = EMBEDDED_STATIC_DIR.get_file(clean) {
        embedded.contents().to_vec()
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let etag = format!(
        "\"{}\"",
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );

    if req_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == etag)
    {
        return StatusCode::NOT_MODIFIED.into_response();
    }

    let content_type = content_type_for(clean);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, STATIC_CACHE)
        .header(header::ETAG, etag)
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn content_type_for(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") || path.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

async fn api_health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "name": "hqcode",
        "version": state.version,
    }))
}

#[derive(Serialize)]
struct AuthSessionStatus {
    authenticated: bool,
    username: String,
    initial_password_pending: bool,
}

async fn api_auth_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Json<AuthSessionStatus> {
    let authenticated = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookie| state.auth.extract_cookie_value(cookie))
        .is_some_and(|token| verify_cookie_is_active(&state, token));

    Json(AuthSessionStatus {
        authenticated,
        username: state.auth.username(),
        initial_password_pending: state.auth.has_initial_password_pending(),
    })
}

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

async fn api_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<LoginRequest>,
) -> ApiResult<Response> {
    let valid = state
        .auth
        .verify_login(payload.username.trim(), &payload.password)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;
    if !valid {
        return Err(AppError::unauthorized("invalid username or password"));
    }

    let token = state
        .auth
        .issue_session_cookie(payload.username.trim())
        .map_err(|e| AppError::internal(e.to_string()))?;
    let is_https = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|p| p.eq_ignore_ascii_case("https"));

    let mut resp = Json(serde_json::json!({
        "ok": true,
        "username": payload.username.trim(),
    }))
    .into_response();
    if let Ok(val) = HeaderValue::from_str(&state.auth.build_set_cookie_header(&token, is_https)) {
        resp.headers_mut().insert(header::SET_COOKIE, val);
    }
    Ok(resp)
}

async fn api_logout(State(state): State<AppState>) -> Response {
    let mut resp = Json(serde_json::json!({ "ok": true })).into_response();
    if let Ok(val) = HeaderValue::from_str(&state.auth.build_clear_cookie_header()) {
        resp.headers_mut().insert(header::SET_COOKIE, val);
    }
    resp
}

#[derive(Deserialize)]
struct ChangePasswordRequest {
    current_password: String,
    #[serde(default)]
    new_username: Option<String>,
    new_password: String,
}

async fn api_change_password(
    State(state): State<AppState>,
    Json(payload): Json<ChangePasswordRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .auth
        .change_password(
            &payload.current_password,
            payload.new_username.as_deref(),
            &payload.new_password,
        )
        .await
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn api_detected_agents() -> Json<Vec<DetectedAgent>> {
    let detected = tokio::task::spawn_blocking(detect_installed_agents)
        .await
        .unwrap_or_default();
    Json(detected)
}

#[derive(Deserialize, Default)]
struct PathQuery {
    #[serde(default)]
    path: String,
}

async fn api_list_entries(
    State(state): State<AppState>,
    Query(query): Query<PathQuery>,
) -> ApiResult<Json<hq_workspace::DirectoryResponse>> {
    let cfg = state
        .workspace
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let res = list_entries(&cfg, &query.path)
        .await
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(res))
}

async fn api_read_file(
    State(state): State<AppState>,
    Query(query): Query<PathQuery>,
) -> ApiResult<Json<hq_workspace::FileResponse>> {
    let cfg = state
        .workspace
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let res = read_utf8_file(&cfg, &query.path)
        .await
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(res))
}

#[derive(Deserialize)]
struct SaveFileRequest {
    path: String,
    content: String,
}

async fn api_save_file(
    State(state): State<AppState>,
    Json(payload): Json<SaveFileRequest>,
) -> ApiResult<Json<hq_workspace::SaveFileResponse>> {
    let cfg = state
        .workspace
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let res = write_utf8_file(&cfg, &payload.path, &payload.content)
        .await
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(res))
}

#[derive(Deserialize)]
struct RenameRequest {
    path: String,
    name: String,
}

async fn api_rename_path(
    State(state): State<AppState>,
    Json(payload): Json<RenameRequest>,
) -> ApiResult<Json<hq_workspace::RenamePathResponse>> {
    let cfg = state
        .workspace
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let res = rename_entry(&cfg, &payload.path, &payload.name)
        .await
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(res))
}

fn read_linux_rss_kb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb_str = rest.split_whitespace().next()?;
                return kb_str.parse::<u64>().ok();
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

async fn api_system_info(State(state): State<AppState>) -> Json<serde_json::Value> {
    let sessions = state.terminal.list_sessions();
    let active_sessions = sessions.iter().filter(|s| s.alive).count();
    Json(serde_json::json!({
        "version": state.version,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "rss_kb": read_linux_rss_kb(),
        "uptime_secs": state.started_at.elapsed().as_secs(),
        "active_sessions": active_sessions,
    }))
}

fn resolve_git_target_dir(
    state: &AppState,
    session_id: Option<&str>,
    path: Option<&str>,
) -> std::result::Result<PathBuf, AppError> {
    let cfg = state
        .workspace
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    if let Some(sid) = session_id.map(str::trim).filter(|s| !s.is_empty())
        && let Some(cwd) = state.terminal.get_session_cwd(sid)
        && let Ok(resolved) = resolve_directory_path(&cfg, &cwd.to_string_lossy())
    {
        return Ok(resolved);
    }
    if let Some(rel) = path.map(str::trim).filter(|s| !s.is_empty()) {
        return resolve_directory_path(&cfg, rel).map_err(|e| AppError::bad_request(e.to_string()));
    }
    Ok(cfg.root)
}

#[derive(Deserialize, Default)]
struct GitTargetQuery {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

async fn api_git_status(
    State(state): State<AppState>,
    Query(query): Query<GitTargetQuery>,
) -> ApiResult<Json<hq_workspace::GitStatusSummary>> {
    let target =
        resolve_git_target_dir(&state, query.session_id.as_deref(), query.path.as_deref())?;
    let summary = git_status(&target)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Json(summary))
}

#[derive(Deserialize, Default)]
struct DiffQuery {
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

async fn api_git_diff(
    State(state): State<AppState>,
    Query(query): Query<DiffQuery>,
) -> ApiResult<Json<hq_workspace::GitDiffResponse>> {
    let target =
        resolve_git_target_dir(&state, query.session_id.as_deref(), query.path.as_deref())?;
    let diff = git_diff(&target, query.file.as_deref())
        .await
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(diff))
}

#[derive(Deserialize)]
struct RevertFileRequest {
    file: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

async fn api_git_revert(
    State(state): State<AppState>,
    Json(payload): Json<RevertFileRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let target = resolve_git_target_dir(
        &state,
        payload.session_id.as_deref(),
        payload.path.as_deref(),
    )?;
    git_revert_file(&target, &payload.file)
        .await
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct CommitRequest {
    message: String,
    #[serde(default)]
    files: Option<Vec<String>>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

async fn api_git_commit(
    State(state): State<AppState>,
    Json(payload): Json<CommitRequest>,
) -> ApiResult<Json<hq_workspace::GitCommitResult>> {
    let target = resolve_git_target_dir(
        &state,
        payload.session_id.as_deref(),
        payload.path.as_deref(),
    )?;
    let res = git_commit(&target, &payload.message, payload.files)
        .await
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(res))
}

async fn api_list_sessions(State(state): State<AppState>) -> Json<serde_json::Value> {
    let sessions = state.terminal.list_sessions();
    Json(serde_json::json!({ "sessions": sessions }))
}

#[derive(Deserialize, Default)]
struct SearchSessionsQuery {
    #[serde(default)]
    q: String,
}

async fn api_search_sessions(
    State(state): State<AppState>,
    Query(query): Query<SearchSessionsQuery>,
) -> Json<serde_json::Value> {
    let matches = state.terminal.search_sessions(&query.q);
    Json(serde_json::json!({ "matches": matches }))
}

async fn api_create_session(
    State(state): State<AppState>,
    Json(mut req): Json<CreateSessionRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if let Some(raw_cwd) = req.cwd.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let cfg = state
            .workspace
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let resolved = resolve_directory_path(&cfg, raw_cwd)
            .map_err(|e| AppError::bad_request(e.to_string()))?;
        req.cwd = Some(resolved.to_string_lossy().into_owned());
    }
    let terminal = state.terminal.clone();
    let meta = tokio::task::spawn_blocking(move || terminal.create_session(req))
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({ "session": meta })))
}

#[derive(Deserialize)]
struct RenameSessionRequest {
    name: String,
}

async fn api_rename_session(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<RenameSessionRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let session = state
        .terminal
        .rename_session(&id, &req.name)
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({ "session": session })))
}

async fn api_close_session(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let terminal = state.terminal.clone();
    tokio::task::spawn_blocking(move || terminal.close_session(&id))
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
        .map_err(|e| AppError::not_found(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct InputRequest {
    data: String,
}

async fn api_send_input(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<InputRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let terminal = state.terminal.clone();
    tokio::task::spawn_blocking(move || terminal.send_input(&id, &req.data))
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn api_send_enter(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let terminal = state.terminal.clone();
    tokio::task::spawn_blocking(move || terminal.send_enter(&id))
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn ws_terminal(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    let (client_id, snap) = state
        .terminal
        .subscribe(&id)
        .map_err(|e| AppError::not_found(e.to_string()))?;
    Ok(ws.on_upgrade(move |socket| handle_ws(state, id, client_id, snap, socket)))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WsClientMsg {
    Input { data: String },
    PacedInput { data: String },
    Enter,
    Resize { cols: u16, rows: u16 },
}

async fn handle_ws(
    state: AppState,
    session_id: String,
    client_id: u64,
    mut snap: hq_tmux::SessionSnapshot,
    socket: WebSocket,
) {
    let (mut sender, mut receiver) = socket.split();

    if !snap.replay_bytes.is_empty() {
        let _ = sender.send(Message::Binary(snap.replay_bytes.into())).await;
    }
    let ready_msg = serde_json::json!({ "type": "ready", "alive": snap.alive }).to_string();
    if sender.send(Message::Text(ready_msg.into())).await.is_err() {
        state.terminal.unregister_client(&session_id, client_id);
        return;
    }

    let send_task = tokio::spawn(async move {
        loop {
            match snap.receiver.recv().await {
                Ok(TerminalStreamEvent::Output(chunk)) => {
                    if chunk.seq <= snap.replay_end_seq {
                        continue;
                    }
                    if sender
                        .send(Message::Binary(chunk.bytes.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(TerminalStreamEvent::ViewportSize(size)) => {
                    let msg = serde_json::json!({
                        "type": "resize",
                        "cols": size.cols,
                        "rows": size.rows,
                    })
                    .to_string();
                    if sender.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
                Ok(TerminalStreamEvent::Closed) => {
                    let _ = sender
                        .send(Message::Text(
                            serde_json::json!({ "type": "closed" }).to_string().into(),
                        ))
                        .await;
                    break;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!("terminal websocket lagged by {skipped} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(txt) => {
                if let Ok(parsed) = serde_json::from_str::<WsClientMsg>(&txt) {
                    match parsed {
                        WsClientMsg::Input { data } => {
                            let _ = state.terminal.write_raw_input(&session_id, &data);
                        }
                        WsClientMsg::PacedInput { data } => {
                            let terminal = state.terminal.clone();
                            let sid = session_id.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                terminal.send_input(&sid, &data)
                            })
                            .await;
                        }
                        WsClientMsg::Enter => {
                            let terminal = state.terminal.clone();
                            let sid = session_id.clone();
                            let _ = tokio::task::spawn_blocking(move || terminal.send_enter(&sid))
                                .await;
                        }
                        WsClientMsg::Resize { cols, rows } => {
                            let _ =
                                state
                                    .terminal
                                    .resize_client(&session_id, client_id, cols, rows);
                        }
                    }
                }
            }
            Message::Binary(bytes) => {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    let _ = state.terminal.write_raw_input(&session_id, text);
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    send_task.abort();
    state.terminal.unregister_client(&session_id, client_id);
}

async fn api_list_presets(State(state): State<AppState>) -> Json<serde_json::Value> {
    let presets: Vec<ModelPreset> = state
        .presets
        .list()
        .into_iter()
        .map(|mut p| {
            if !p.api_key.is_empty() {
                p.api_key = "********".to_string();
            }
            p
        })
        .collect();
    Json(serde_json::json!({ "presets": presets }))
}

async fn api_upsert_preset(
    State(state): State<AppState>,
    Json(preset): Json<ModelPreset>,
) -> ApiResult<Json<serde_json::Value>> {
    let saved = state
        .presets
        .upsert(preset)
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({ "preset": saved })))
}

async fn api_delete_preset(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let removed = state
        .presets
        .remove(&id)
        .map_err(|e| AppError::internal(e.to_string()))?;
    if !removed {
        return Err(AppError::not_found("preset not found"));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn api_apply_preset(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let preset = state
        .presets
        .resolve(&id)
        .ok_or_else(|| AppError::not_found("preset not found"))?;
    apply_preset(&state.user_home, &preset).map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn api_active_presets(State(state): State<AppState>) -> Json<hq_presets::ActiveCliConfigs> {
    Json(inspect_active_configs(&state.user_home))
}

#[derive(Deserialize)]
struct ImportPresetRequest {
    kind: PresetKind,
    #[serde(default)]
    name: Option<String>,
}

async fn api_import_preset(
    State(state): State<AppState>,
    Json(payload): Json<ImportPresetRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let imported = import_current_config(payload.kind, &state.user_home, payload.name.as_deref())
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    let mut saved = state
        .presets
        .upsert(imported)
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    if !saved.api_key.is_empty() {
        saved.api_key = "********".to_string();
    }
    Ok(Json(serde_json::json!({ "preset": saved })))
}

struct OptionalPeerAddr(Option<SocketAddr>);

impl<S> FromRequestParts<S> for OptionalPeerAddr
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        let addr = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0);
        Ok(Self(addr))
    }
}

#[derive(Deserialize)]
struct IssueTicketRequest {
    #[serde(default)]
    endpoint_url: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    ttl_secs: Option<u64>,
}

async fn api_remote_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let endpoints = discover_endpoints(state.bind_addr, state.public_url.as_deref());
    let is_loopback_only = endpoints.iter().all(|e| !e.reachable_from_phone);
    Json(serde_json::json!({
        "bind_addr": state.bind_addr.to_string(),
        "is_loopback_only": is_loopback_only,
        "public_url": state.public_url,
        "endpoints": endpoints,
        "active_tickets": state.remote.list_active_tickets(),
        "devices": state.remote.list_devices(),
    }))
}

async fn api_remote_issue_ticket(
    State(state): State<AppState>,
    Json(payload): Json<IssueTicketRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let endpoints = discover_endpoints(state.bind_addr, state.public_url.as_deref());
    let chosen_url = payload
        .endpoint_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| preferred_phone_endpoint(&endpoints).map(|ep| ep.url.clone()))
        .or_else(|| endpoints.first().map(|ep| ep.url.clone()))
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", state.bind_addr.port()));

    let issued = state
        .remote
        .issue_ticket(&chosen_url, payload.label.as_deref(), payload.ttl_secs)
        .map_err(|e| AppError::bad_request(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "ticket": issued.ticket,
        "pair_url": issued.ticket.pair_url,
        "qr_svg": issued.qr_svg,
        "qr_terminal": issued.qr_terminal,
        "expires_in_secs": issued.ticket.expires_in_secs,
    })))
}

async fn api_remote_revoke_ticket(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let revoked = state.remote.revoke_ticket(&id);
    if !revoked {
        return Err(AppError::not_found("pairing ticket not found"));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn api_remote_revoke_device(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let revoked = state
        .remote
        .revoke_device(&id)
        .map_err(|e| AppError::internal(e.to_string()))?;
    if !revoked {
        return Err(AppError::not_found("paired device not found"));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct RedeemTicketRequest {
    code: String,
    #[serde(default)]
    device_name: Option<String>,
}

async fn api_remote_redeem(
    State(state): State<AppState>,
    OptionalPeerAddr(peer_addr): OptionalPeerAddr,
    headers: HeaderMap,
    Json(payload): Json<RedeemTicketRequest>,
) -> ApiResult<Response> {
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let is_loopback_peer = peer_addr.is_some_and(|a| a.ip().is_loopback());
    let remote_ip = if is_loopback_peer {
        headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| peer_addr.map(|a| a.ip().to_string()).unwrap_or_default())
    } else {
        peer_addr.map(|a| a.ip().to_string()).unwrap_or_default()
    };

    let device = state
        .remote
        .redeem_ticket(
            &payload.code,
            payload.device_name.as_deref(),
            user_agent,
            &remote_ip,
        )
        .map_err(|e| AppError::unauthorized(e.to_string()))?;

    let username = state.auth.username();
    let token = state
        .auth
        .issue_session_cookie_for_device(&username, &device.id)
        .map_err(|e| AppError::internal(e.to_string()))?;
    state.auth.clear_initial_password();

    let is_https = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|p| p.eq_ignore_ascii_case("https"));

    let mut resp = Json(serde_json::json!({
        "ok": true,
        "username": username,
        "device": device,
    }))
    .into_response();
    if let Ok(val) = HeaderValue::from_str(&state.auth.build_set_cookie_header(&token, is_https)) {
        resp.headers_mut().insert(header::SET_COOKIE, val);
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_and_auth_gate_work() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_dir = std::env::temp_dir().join(format!("hq-srv-test-{nonce}"));
        std::fs::create_dir_all(&data_dir).unwrap();
        let user = resolve_current_user().unwrap();
        let auth = AuthService::initialize(&data_dir, AuthConfig::for_app("hqcode")).unwrap();
        let remote =
            RemoteConnectService::initialize(&data_dir, RemoteConnectConfig::for_app("hqcode"))
                .unwrap();
        let state = AppState {
            version: "0.1.0".to_string(),
            started_at: Instant::now(),
            static_dir: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/static")),
            workspace: Arc::new(RwLock::new(WorkspaceConfig::new(PathBuf::from("/tmp")))),
            auth: auth.clone(),
            terminal: TerminalManager::new(
                TmuxNamespace::new("hqcode-srv-test"),
                data_dir.join("sessions.json"),
                PathBuf::from("/tmp"),
                user.clone(),
            ),
            presets: PresetStore::load(&data_dir, "hqcode"),
            user_home: user.home,
            bind_addr: "127.0.0.1:9900".parse().unwrap(),
            public_url: None,
            remote,
        };
        let app = build_router(state);

        let health = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);

        let unauth = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/entries")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

        let token = auth.issue_session_cookie("hqcode").unwrap();
        let cookie = format!("hqcode_session={token}");

        let sys = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/system/info")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(sys.status(), StatusCode::OK);

        let search = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/terminal/sessions/search?q=hello")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(search.status(), StatusCode::OK);

        let git_st = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/git/status?path=.")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(git_st.status(), StatusCode::OK);

        // Remote QR pairing end-to-end closed loop: issue -> redeem -> auth OK -> replay rejected -> device revoke -> 401.
        let issue_resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/remote/tickets")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"endpoint_url":"http://192.168.1.9:9900","label":"Phone"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(issue_resp.status(), StatusCode::OK);
        let issue_bytes = axum::body::to_bytes(issue_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let issue_json: serde_json::Value = serde_json::from_slice(&issue_bytes).unwrap();
        let pair_url = issue_json["pair_url"].as_str().unwrap();
        let code = pair_url.split("code=").nth(1).unwrap().to_string();
        assert!(issue_json["qr_svg"].as_str().unwrap().contains("<svg"));

        let redeem_resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/remote/redeem")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::USER_AGENT, "iPhone Safari")
                    .body(Body::from(format!(r#"{{"code":"{code}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(redeem_resp.status(), StatusCode::OK);
        let set_cookie = redeem_resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let redeem_bytes = axum::body::to_bytes(redeem_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let redeem_json: serde_json::Value = serde_json::from_slice(&redeem_bytes).unwrap();
        let device_id = redeem_json["device"]["id"].as_str().unwrap().to_string();

        // Paired device cookie accesses protected API.
        let paired_sys = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/system/info")
                    .header(header::COOKIE, &set_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(paired_sys.status(), StatusCode::OK);

        // Replaying the one-time code fails.
        let replay_resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/remote/redeem")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"code":"{code}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay_resp.status(), StatusCode::UNAUTHORIZED);

        // Revoking the device immediately invalidates its cookie.
        let revoke_resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("DELETE")
                    .uri(format!("/api/remote/devices/{device_id}"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoke_resp.status(), StatusCode::OK);

        let revoked_sys = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/system/info")
                    .header(header::COOKIE, &set_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoked_sys.status(), StatusCode::UNAUTHORIZED);

        let _ = std::fs::remove_dir_all(&data_dir);
    }
}
