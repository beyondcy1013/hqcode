//! Parameterized remote endpoint discovery, one-time QR pairing tickets,
//! SVG/terminal QR rendering, and revocable paired-device persistence.

use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use qrcode::{
    QrCode,
    render::{svg, unicode::Dense1x2},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

const DEFAULT_TICKET_TTL_SECS: u64 = 300;
const MIN_TICKET_TTL_SECS: u64 = 30;
const MAX_TICKET_TTL_SECS: u64 = 3600;
const DEVICE_TOUCH_PERSIST_INTERVAL_SECS: u64 = 60;

#[derive(Debug, Clone)]
pub struct RemoteConnectConfig {
    pub file_prefix: String,
    pub pair_path: String,
    pub default_ttl_secs: u64,
    pub max_active_tickets: usize,
}

impl RemoteConnectConfig {
    pub fn for_app(prefix: &str) -> Self {
        Self {
            file_prefix: prefix.to_string(),
            pair_path: "/pair".to_string(),
            default_ttl_secs: DEFAULT_TICKET_TTL_SECS,
            max_active_tickets: 16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointKind {
    Public,
    Tailscale,
    Lan,
    Loopback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteEndpoint {
    pub kind: EndpointKind,
    pub label: String,
    pub url: String,
    pub reachable_from_phone: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingTicketInfo {
    pub id: String,
    pub label: String,
    pub target_url: String,
    pub pair_url: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub expires_in_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuedPairing {
    pub ticket: PairingTicketInfo,
    pub code: String,
    pub qr_svg: String,
    pub qr_terminal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub id: String,
    pub label: String,
    pub user_agent: String,
    pub remote_ip: String,
    pub paired_at: u64,
    pub last_seen_at: u64,
    pub revoked: bool,
}

#[derive(Debug, Clone)]
struct StoredTicket {
    info: PairingTicketInfo,
    code: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedDevices {
    version: u8,
    devices: Vec<PairedDevice>,
}

#[derive(Clone)]
pub struct RemoteConnectService {
    devices_path: PathBuf,
    config: RemoteConnectConfig,
    tickets: Arc<Mutex<Vec<StoredTicket>>>,
    devices: Arc<RwLock<HashMap<String, PairedDevice>>>,
    persist_lock: Arc<Mutex<()>>,
}

impl RemoteConnectService {
    pub fn initialize(data_dir: &Path, config: RemoteConnectConfig) -> Result<Self> {
        let devices_path = data_dir.join(format!(".{}-paired-devices.json", config.file_prefix));
        let persisted = hq_fs_atomic::read_private_json::<PersistedDevices>(&devices_path)
            .unwrap_or(PersistedDevices {
                version: 1,
                devices: Vec::new(),
            });
        let map = persisted
            .devices
            .into_iter()
            .map(|d| (d.id.clone(), d))
            .collect();

        Ok(Self {
            devices_path,
            config,
            tickets: Arc::new(Mutex::new(Vec::new())),
            devices: Arc::new(RwLock::new(map)),
            persist_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn issue_ticket(
        &self,
        base_url: &str,
        label: Option<&str>,
        ttl_secs: Option<u64>,
    ) -> Result<IssuedPairing> {
        let trimmed_base = normalize_base_url(base_url)?;
        let ttl = ttl_secs
            .unwrap_or(self.config.default_ttl_secs)
            .clamp(MIN_TICKET_TTL_SECS, MAX_TICKET_TTL_SECS);
        let now = now_unix_secs();
        let expires_at = now.saturating_add(ttl);

        let id = format!("tk-{}", random_urlsafe(6));
        let code = random_urlsafe(24);
        let pair_path = if self.config.pair_path.starts_with('/') {
            self.config.pair_path.as_str()
        } else {
            "/pair"
        };
        let pair_url = format!("{trimmed_base}{pair_path}?code={code}");
        let ticket_label = label
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Mobile Pairing")
            .to_string();

        let qr_svg = render_qr_svg(&pair_url)?;
        let qr_terminal = render_qr_terminal(&pair_url, true)?;

        let info = PairingTicketInfo {
            id,
            label: ticket_label,
            target_url: trimmed_base,
            pair_url,
            created_at: now,
            expires_at,
            expires_in_secs: ttl,
        };

        {
            let mut guard = self.tickets.lock().unwrap_or_else(|p| p.into_inner());
            guard.retain(|t| t.info.expires_at > now);
            while guard.len() >= self.config.max_active_tickets.max(1) {
                guard.remove(0);
            }
            guard.push(StoredTicket {
                info: info.clone(),
                code: code.clone(),
            });
        }

        Ok(IssuedPairing {
            ticket: info,
            code,
            qr_svg,
            qr_terminal,
        })
    }

    pub fn list_active_tickets(&self) -> Vec<PairingTicketInfo> {
        let now = now_unix_secs();
        let mut guard = self.tickets.lock().unwrap_or_else(|p| p.into_inner());
        guard.retain(|t| t.info.expires_at > now);
        guard
            .iter()
            .map(|t| {
                let mut info = t.info.clone();
                info.expires_in_secs = info.expires_at.saturating_sub(now);
                info
            })
            .collect()
    }

    pub fn revoke_ticket(&self, id: &str) -> bool {
        let mut guard = self.tickets.lock().unwrap_or_else(|p| p.into_inner());
        let before = guard.len();
        guard.retain(|t| t.info.id != id);
        guard.len() != before
    }

    pub fn redeem_ticket(
        &self,
        code: &str,
        device_label: Option<&str>,
        user_agent: &str,
        remote_ip: &str,
    ) -> Result<PairedDevice> {
        let candidate = code.trim();
        if candidate.is_empty() {
            bail!("pairing code is required");
        }
        let now = now_unix_secs();

        let matched = {
            let mut guard = self.tickets.lock().unwrap_or_else(|p| p.into_inner());
            guard.retain(|t| t.info.expires_at > now);
            let idx = guard.iter().position(|t| {
                let a = t.code.as_bytes();
                let b = candidate.as_bytes();
                a.len() == b.len() && bool::from(a.ct_eq(b))
            });
            idx.map(|i| guard.remove(i))
        };

        let Some(ticket) = matched else {
            bail!("invalid or expired pairing code");
        };

        let label = device_label
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| summarize_device_label(user_agent, &ticket.info.label));

        let device = PairedDevice {
            id: format!("dev-{}", random_urlsafe(8)),
            label,
            user_agent: truncate_str(user_agent.trim(), 160),
            remote_ip: truncate_str(remote_ip.trim(), 64),
            paired_at: now,
            last_seen_at: now,
            revoked: false,
        };

        let _persist = self.persist_lock.lock().unwrap_or_else(|p| p.into_inner());
        {
            let mut map = self.devices.write().unwrap_or_else(|p| p.into_inner());
            map.insert(device.id.clone(), device.clone());
            self.write_devices_locked(&map)?;
        }

        Ok(device)
    }

    pub fn list_devices(&self) -> Vec<PairedDevice> {
        let map = self.devices.read().unwrap_or_else(|p| p.into_inner());
        let mut list: Vec<PairedDevice> = map.values().cloned().collect();
        list.sort_by(|a, b| b.paired_at.cmp(&a.paired_at).then_with(|| a.id.cmp(&b.id)));
        list
    }

    pub fn is_device_active(&self, device_id: &str) -> bool {
        let map = self.devices.read().unwrap_or_else(|p| p.into_inner());
        map.get(device_id).is_some_and(|d| !d.revoked)
    }

    pub fn touch_device(&self, device_id: &str) {
        let now = now_unix_secs();
        let should_persist = {
            let mut map = self.devices.write().unwrap_or_else(|p| p.into_inner());
            let Some(dev) = map.get_mut(device_id) else {
                return;
            };
            if dev.revoked {
                return;
            }
            let elapsed = now.saturating_sub(dev.last_seen_at);
            dev.last_seen_at = now;
            elapsed >= DEVICE_TOUCH_PERSIST_INTERVAL_SECS
        };

        if should_persist && let Ok(_persist) = self.persist_lock.try_lock() {
            let map = self.devices.read().unwrap_or_else(|p| p.into_inner());
            let _ = self.write_devices_locked(&map);
        }
    }

    pub fn revoke_device(&self, device_id: &str) -> Result<bool> {
        let _persist = self.persist_lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut map = self.devices.write().unwrap_or_else(|p| p.into_inner());
        let Some(dev) = map.get_mut(device_id) else {
            return Ok(false);
        };
        if !dev.revoked {
            dev.revoked = true;
            self.write_devices_locked(&map)?;
        }
        Ok(true)
    }

    fn write_devices_locked(&self, map: &HashMap<String, PairedDevice>) -> Result<()> {
        let mut devices: Vec<PairedDevice> = map.values().cloned().collect();
        devices.sort_by(|a, b| a.paired_at.cmp(&b.paired_at).then_with(|| a.id.cmp(&b.id)));
        let payload = PersistedDevices {
            version: 1,
            devices,
        };
        hq_fs_atomic::write_private_json(&self.devices_path, &payload).with_context(|| {
            format!(
                "failed to persist paired devices {}",
                self.devices_path.display()
            )
        })
    }
}

pub fn discover_endpoints(bind_addr: SocketAddr, public_url: Option<&str>) -> Vec<RemoteEndpoint> {
    let mut result = Vec::new();
    let mut seen_urls = HashSet::new();
    let port = bind_addr.port();

    if let Some(raw_pub) = public_url.map(str::trim).filter(|s| !s.is_empty())
        && let Ok(norm) = normalize_base_url(raw_pub)
        && seen_urls.insert(norm.clone())
    {
        result.push(RemoteEndpoint {
            kind: EndpointKind::Public,
            label: "Public URL".to_string(),
            url: norm,
            reachable_from_phone: true,
        });
    }

    let ip = bind_addr.ip();
    if ip.is_unspecified() {
        let mut candidates = discover_non_loopback_ipv4s();
        if let Some(primary) = probe_primary_outbound_ipv4()
            && !candidates.contains(&primary)
        {
            candidates.insert(0, primary);
        }
        // Sort priority: Tailscale (100.64/10) or primary RFC1918 first, link-local excluded.
        let primary_opt = probe_primary_outbound_ipv4();
        candidates.sort_by_key(|addr| {
            let is_primary = primary_opt == Some(*addr);
            let kind = classify_ipv4(*addr);
            (kind, !is_primary, *addr)
        });
        candidates.dedup();

        for addr in candidates {
            let kind = classify_ipv4(addr);
            let url = format_http_url(IpAddr::V4(addr), port);
            if !seen_urls.insert(url.clone()) {
                continue;
            }
            let label = match kind {
                EndpointKind::Tailscale => format!("Tailscale ({addr})"),
                EndpointKind::Lan => format!("LAN ({addr})"),
                EndpointKind::Public => format!("IP ({addr})"),
                EndpointKind::Loopback => format!("Localhost ({addr})"),
            };
            result.push(RemoteEndpoint {
                kind,
                label,
                url,
                reachable_from_phone: kind != EndpointKind::Loopback,
            });
        }
    } else if !ip.is_loopback() {
        let kind = match ip {
            IpAddr::V4(v4) => classify_ipv4(v4),
            IpAddr::V6(_) => EndpointKind::Lan,
        };
        let url = format_http_url(ip, port);
        if seen_urls.insert(url.clone()) {
            let label = match kind {
                EndpointKind::Tailscale => format!("Tailscale ({ip})"),
                EndpointKind::Lan => format!("LAN ({ip})"),
                EndpointKind::Public => format!("Bound ({ip})"),
                EndpointKind::Loopback => format!("Localhost ({ip})"),
            };
            result.push(RemoteEndpoint {
                kind,
                label,
                url,
                reachable_from_phone: true,
            });
        }
    }

    let loopback_url = format!("http://127.0.0.1:{port}");
    if seen_urls.insert(loopback_url.clone()) {
        result.push(RemoteEndpoint {
            kind: EndpointKind::Loopback,
            label: format!("Localhost (127.0.0.1:{port})"),
            url: loopback_url,
            reachable_from_phone: false,
        });
    }

    result
}

pub fn preferred_phone_endpoint(endpoints: &[RemoteEndpoint]) -> Option<&RemoteEndpoint> {
    endpoints.iter().find(|ep| ep.reachable_from_phone)
}

pub fn render_qr_svg(content: &str) -> Result<String> {
    let code = QrCode::new(content.as_bytes()).context("failed to encode QR SVG")?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(220, 220)
        .dark_color(svg::Color("#0a0c10"))
        .light_color(svg::Color("#ffffff"))
        .quiet_zone(true)
        .build())
}

pub fn render_qr_terminal(content: &str, invert_for_dark_bg: bool) -> Result<String> {
    let code = QrCode::new(content.as_bytes()).context("failed to encode terminal QR")?;
    let mut renderer = code.render::<Dense1x2>();
    if invert_for_dark_bg {
        renderer
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark);
    } else {
        renderer
            .dark_color(Dense1x2::Dark)
            .light_color(Dense1x2::Light);
    }
    Ok(renderer.quiet_zone(true).build())
}

fn format_http_url(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(v4) => format!("http://{v4}:{port}"),
        IpAddr::V6(v6) => format!("http://[{v6}]:{port}"),
    }
}

fn normalize_base_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        bail!("endpoint URL cannot be empty");
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        bail!("endpoint URL must start with http:// or https://");
    }
    Ok(trimmed.to_string())
}

fn classify_ipv4(ip: Ipv4Addr) -> EndpointKind {
    let o = ip.octets();
    if ip.is_loopback() {
        EndpointKind::Loopback
    } else if o[0] == 100 && (64..=127).contains(&o[1]) {
        EndpointKind::Tailscale
    } else if o[0] == 10
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
    {
        EndpointKind::Lan
    } else {
        EndpointKind::Public
    }
}

fn probe_primary_outbound_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let IpAddr::V4(v4) = sock.local_addr().ok()?.ip() else {
        return None;
    };
    if v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() {
        None
    } else {
        Some(v4)
    }
}

#[cfg(unix)]
fn discover_non_loopback_ipv4s() -> Vec<Ipv4Addr> {
    struct IfAddrsGuard(*mut libc::ifaddrs);
    impl Drop for IfAddrsGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { libc::freeifaddrs(self.0) };
            }
        }
    }

    let mut raw: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut raw) } != 0 || raw.is_null() {
        return Vec::new();
    }
    let _guard = IfAddrsGuard(raw);

    let mut out = Vec::new();
    let mut cur = raw;
    while !cur.is_null() {
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        let flags = ifa.ifa_flags as i32;
        if (flags & libc::IFF_UP) == 0 || (flags & libc::IFF_LOOPBACK) != 0 {
            continue;
        }
        if ifa.ifa_addr.is_null() {
            continue;
        }
        let family = unsafe { (*ifa.ifa_addr).sa_family };
        if family != libc::AF_INET as libc::sa_family_t {
            continue;
        }
        let sin = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
        let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
        if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_multicast() {
            continue;
        }
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    out
}

#[cfg(not(unix))]
fn discover_non_loopback_ipv4s() -> Vec<Ipv4Addr> {
    probe_primary_outbound_ipv4().into_iter().collect()
}

fn summarize_device_label(user_agent: &str, fallback: &str) -> String {
    let ua = user_agent.to_ascii_lowercase();
    let os = if ua.contains("iphone") {
        "iPhone"
    } else if ua.contains("ipad") {
        "iPad"
    } else if ua.contains("android") {
        "Android"
    } else if ua.contains("mac os") || ua.contains("macintosh") {
        "macOS"
    } else if ua.contains("windows") {
        "Windows"
    } else if ua.contains("linux") {
        "Linux"
    } else {
        ""
    };

    let browser = if ua.contains("edg/") {
        "Edge"
    } else if ua.contains("chrome") && !ua.contains("edg/") {
        "Chrome"
    } else if ua.contains("safari") && !ua.contains("chrome") {
        "Safari"
    } else if ua.contains("firefox") {
        "Firefox"
    } else {
        ""
    };

    match (os.is_empty(), browser.is_empty()) {
        (false, false) => format!("{os} · {browser}"),
        (false, true) => os.to_string(),
        (true, false) => browser.to_string(),
        (true, true) => fallback.to_string(),
    }
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn random_urlsafe(byte_len: usize) -> String {
    let mut raw = vec![0_u8; byte_len];
    rand::thread_rng().fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
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
    use std::fs;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir()
                .join(format!("hq-remote-{label}-{}-{nonce}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn issue_redeem_once_and_revoke_device_lifecycle() {
        let dir = TempDir::new("lifecycle");
        let svc = RemoteConnectService::initialize(&dir.0, RemoteConnectConfig::for_app("hqcode"))
            .unwrap();

        let issued = svc
            .issue_ticket("http://192.168.1.50:9900/", Some("Phone"), Some(120))
            .unwrap();
        assert!(
            issued
                .ticket
                .pair_url
                .starts_with("http://192.168.1.50:9900/pair?code=")
        );
        assert!(issued.qr_svg.contains("<svg"));
        assert!(!issued.qr_terminal.trim().is_empty());
        assert_eq!(svc.list_active_tickets().len(), 1);

        let dev = svc
            .redeem_ticket(
                &issued.code,
                None,
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) AppleWebKit/605.1.15 Safari/604.1",
                "192.168.1.88",
            )
            .unwrap();
        assert_eq!(dev.label, "iPhone · Safari");
        assert!(svc.is_device_active(&dev.id));
        assert!(svc.list_active_tickets().is_empty());

        // Second redeem of same one-time code must fail.
        assert!(
            svc.redeem_ticket(&issued.code, None, "UA", "192.168.1.88")
                .is_err()
        );

        // Revoking device immediately disables active status and persists across reload.
        assert!(svc.revoke_device(&dev.id).unwrap());
        assert!(!svc.is_device_active(&dev.id));

        let reloaded =
            RemoteConnectService::initialize(&dir.0, RemoteConnectConfig::for_app("hqcode"))
                .unwrap();
        assert!(!reloaded.is_device_active(&dev.id));
        assert_eq!(reloaded.list_devices().len(), 1);
    }

    #[test]
    fn expired_or_revoked_ticket_cannot_be_redeemed() {
        let dir = TempDir::new("ticket-revoke");
        let svc = RemoteConnectService::initialize(&dir.0, RemoteConnectConfig::for_app("webclx"))
            .unwrap();
        let issued = svc
            .issue_ticket("http://127.0.0.1:11111", None, Some(60))
            .unwrap();
        assert!(svc.revoke_ticket(&issued.ticket.id));
        assert!(
            svc.redeem_ticket(&issued.code, None, "Android Chrome", "10.0.0.5")
                .is_err()
        );

        // Manually inject an expired ticket and verify cleanup rejects it.
        {
            let mut guard = svc.tickets.lock().unwrap();
            guard.push(StoredTicket {
                info: PairingTicketInfo {
                    id: "tk-expired".to_string(),
                    label: "Old".to_string(),
                    target_url: "http://127.0.0.1:9900".to_string(),
                    pair_url: "http://127.0.0.1:9900/pair?code=oldcode".to_string(),
                    created_at: 1,
                    expires_at: 2,
                    expires_in_secs: 0,
                },
                code: "oldcode".to_string(),
            });
        }
        assert!(
            svc.redeem_ticket("oldcode", None, "Android", "10.0.0.5")
                .is_err()
        );
    }

    #[test]
    fn endpoint_discovery_prioritizes_public_and_phone_reachable() {
        let bind_loopback: SocketAddr = "127.0.0.1:9900".parse().unwrap();
        let eps = discover_endpoints(bind_loopback, Some("https://hq.example.com/"));
        assert_eq!(eps[0].kind, EndpointKind::Public);
        assert_eq!(eps[0].url, "https://hq.example.com");
        assert!(eps[0].reachable_from_phone);
        assert_eq!(
            preferred_phone_endpoint(&eps).map(|e| e.url.as_str()),
            Some("https://hq.example.com")
        );

        let eps_local_only = discover_endpoints(bind_loopback, None);
        assert_eq!(eps_local_only.len(), 1);
        assert_eq!(eps_local_only[0].kind, EndpointKind::Loopback);
        assert!(preferred_phone_endpoint(&eps_local_only).is_none());
    }
}
