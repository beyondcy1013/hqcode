//! Parameterized single-user authentication, session cookie signing, initial
//! password bootstrap, and loopback local-token authentication.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, Semaphore};

type HmacSha256 = Hmac<Sha256>;

const MAX_CONCURRENT_PASSWORD_VERIFICATIONS: usize = 2;
const INITIAL_PASSWORD_RANDOM_BYTES: usize = 24;

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub cookie_name: String,
    pub file_prefix: String,
    pub default_username: String,
    pub session_ttl_days: u64,
}

impl AuthConfig {
    pub fn for_app(prefix: &str) -> Self {
        Self {
            cookie_name: format!("{prefix}_session"),
            file_prefix: prefix.to_string(),
            default_username: prefix.to_string(),
            session_ttl_days: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCredentials {
    version: u8,
    username: String,
    password_hash: String,
}

#[derive(Clone)]
pub struct AuthService {
    data_dir: PathBuf,
    config: AuthConfig,
    session_secret: Arc<Vec<u8>>,
    local_api_token: Arc<str>,
    credentials: Arc<RwLock<StoredCredentials>>,
    verify_slots: Arc<Semaphore>,
    password_update_lock: Arc<Mutex<()>>,
}

impl AuthService {
    pub fn initialize(data_dir: &Path, config: AuthConfig) -> Result<Self> {
        fs::create_dir_all(data_dir)
            .with_context(|| format!("failed to create auth data dir {}", data_dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(data_dir, fs::Permissions::from_mode(0o700));
        }

        let secret_path = data_dir.join(format!(".{}-session-secret", config.file_prefix));
        let session_secret = load_or_create_secret(&secret_path)?;

        let local_token_path = data_dir.join(format!(".{}-local-token", config.file_prefix));
        let local_api_token: Arc<str> = Arc::from(load_or_create_local_token(&local_token_path)?);

        let credentials_path = data_dir.join(format!(".{}-credentials.json", config.file_prefix));
        let initial_pw_path = data_dir.join(format!(".{}-initial-password", config.file_prefix));
        let credentials = load_or_create_credentials(&credentials_path, &initial_pw_path, &config)?;

        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            config,
            session_secret: Arc::new(session_secret),
            local_api_token,
            credentials: Arc::new(RwLock::new(credentials)),
            verify_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_PASSWORD_VERIFICATIONS)),
            password_update_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn cookie_name(&self) -> &str {
        &self.config.cookie_name
    }

    pub fn local_api_token(&self) -> &str {
        &self.local_api_token
    }

    pub fn username(&self) -> String {
        self.credentials
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .username
            .clone()
    }

    pub fn initial_password_path(&self) -> PathBuf {
        self.data_dir
            .join(format!(".{}-initial-password", self.config.file_prefix))
    }

    pub fn has_initial_password_pending(&self) -> bool {
        self.initial_password_path().exists()
    }

    pub fn verify_local_token(&self, candidate: &str) -> bool {
        let a = candidate.as_bytes();
        let b = self.local_api_token.as_bytes();
        a.len() == b.len() && bool::from(a.ct_eq(b))
    }

    pub async fn verify_login(&self, username: &str, password: &str) -> Result<bool> {
        let creds = self
            .credentials
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if !bool::from(username.as_bytes().ct_eq(creds.username.as_bytes()))
            && username.len() != creds.username.len()
        {
            return Ok(false);
        }
        if username != creds.username {
            return Ok(false);
        }

        let _permit = self
            .verify_slots
            .acquire()
            .await
            .context("verification semaphore closed")?;
        let password_owned = password.to_string();
        let hash_owned = creds.password_hash.clone();
        let verified =
            tokio::task::spawn_blocking(move || verify_argon2(&password_owned, &hash_owned))
                .await
                .context("password verification task panicked")??;

        if verified {
            let _ = fs::remove_file(self.initial_password_path());
        }
        Ok(verified)
    }

    pub async fn change_password(
        &self,
        current_password: &str,
        new_username: Option<&str>,
        new_password: &str,
    ) -> Result<()> {
        if new_password.len() < 8 {
            bail!("new password must be at least 8 characters");
        }
        let _guard = self.password_update_lock.lock().await;
        let current_username = self.username();
        if !self
            .verify_login(&current_username, current_password)
            .await?
        {
            bail!("current password is incorrect");
        }
        let username = new_username
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&current_username)
            .to_string();
        let new_pw = new_password.to_string();
        let password_hash = tokio::task::spawn_blocking(move || hash_argon2(&new_pw))
            .await
            .context("password hashing task panicked")??;

        let updated = StoredCredentials {
            version: 2,
            username,
            password_hash,
        };
        let credentials_path = self
            .data_dir
            .join(format!(".{}-credentials.json", self.config.file_prefix));
        hq_fs_atomic::write_private_json(&credentials_path, &updated)?;
        *self.credentials.write().unwrap_or_else(|p| p.into_inner()) = updated;
        let _ = fs::remove_file(self.initial_password_path());
        Ok(())
    }

    pub fn issue_session_cookie(&self, username: &str) -> Result<String> {
        let now = now_unix_secs();
        let expires = now.saturating_add(self.config.session_ttl_days * 24 * 60 * 60);
        let payload = format!("{username}:{expires}");
        let encoded_payload = URL_SAFE_NO_PAD.encode(payload.as_bytes());
        let sig = sign_payload(&self.session_secret, encoded_payload.as_bytes())?;
        let encoded_sig = URL_SAFE_NO_PAD.encode(sig);
        Ok(format!("{encoded_payload}.{encoded_sig}"))
    }

    pub fn verify_session_cookie(&self, token: &str) -> Option<String> {
        let (encoded_payload, encoded_sig) = token.split_once('.')?;
        let provided_sig = URL_SAFE_NO_PAD.decode(encoded_sig).ok()?;
        let expected_sig = sign_payload(&self.session_secret, encoded_payload.as_bytes()).ok()?;
        if provided_sig.len() != expected_sig.len()
            || !bool::from(provided_sig.ct_eq(&expected_sig))
        {
            return None;
        }
        let payload_bytes = URL_SAFE_NO_PAD.decode(encoded_payload).ok()?;
        let payload = String::from_utf8(payload_bytes).ok()?;
        let (username, expires_str) = payload.rsplit_once(':')?;
        let expires: u64 = expires_str.parse().ok()?;
        if expires <= now_unix_secs() {
            return None;
        }
        if username != self.username() {
            return None;
        }
        Some(username.to_string())
    }

    pub fn build_set_cookie_header(&self, token: &str, secure: bool) -> String {
        let max_age = Duration::from_secs(self.config.session_ttl_days * 24 * 60 * 60).as_secs();
        let secure_attr = if secure { "; Secure" } else { "" };
        format!(
            "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
            self.config.cookie_name, token, max_age, secure_attr
        )
    }

    pub fn build_clear_cookie_header(&self) -> String {
        format!(
            "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
            self.config.cookie_name
        )
    }

    pub fn extract_cookie_value<'a>(&self, cookie_header: &'a str) -> Option<&'a str> {
        for part in cookie_header.split(';') {
            let trimmed = part.trim();
            if let Some((name, val)) = trimmed.split_once('=')
                && name.trim() == self.config.cookie_name
            {
                return Some(val.trim());
            }
        }
        None
    }
}

fn sign_payload(secret: &[u8], payload: &[u8]) -> Result<Vec<u8>> {
    let mut mac = HmacSha256::new_from_slice(secret).context("invalid HMAC key")?;
    mac.update(payload);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn load_or_create_secret(path: &Path) -> Result<Vec<u8>> {
    if hq_fs_atomic::validate_optional_regular(path)? {
        let bytes = fs::read(path)?;
        if bytes.len() >= 32 {
            hq_fs_atomic::tighten_permissions(path)?;
            return Ok(bytes);
        }
    }
    let mut secret = vec![0_u8; 64];
    rand::thread_rng().fill_bytes(&mut secret);
    hq_fs_atomic::write_private_atomic_with_parent(path, &secret)?;
    Ok(secret)
}

fn load_or_create_local_token(path: &Path) -> Result<String> {
    if hq_fs_atomic::validate_optional_regular(path)? {
        let content = fs::read_to_string(path)?;
        let trimmed = content.trim().to_string();
        if trimmed.len() >= 32 {
            hq_fs_atomic::tighten_permissions(path)?;
            return Ok(trimmed);
        }
    }
    let mut raw = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let token = hex::encode(raw);
    hq_fs_atomic::write_private_atomic_with_parent(path, format!("{token}\n").as_bytes())?;
    Ok(token)
}

fn load_or_create_credentials(
    credentials_path: &Path,
    initial_pw_path: &Path,
    config: &AuthConfig,
) -> Result<StoredCredentials> {
    if let Some(existing) = hq_fs_atomic::read_private_json::<StoredCredentials>(credentials_path)
        && !existing.username.is_empty()
        && !existing.password_hash.is_empty()
    {
        return Ok(existing);
    }

    let mut raw = [0_u8; INITIAL_PASSWORD_RANDOM_BYTES];
    rand::thread_rng().fill_bytes(&mut raw);
    let initial_password = URL_SAFE_NO_PAD.encode(raw);
    let password_hash = hash_argon2(&initial_password)?;
    let creds = StoredCredentials {
        version: 2,
        username: config.default_username.clone(),
        password_hash,
    };
    hq_fs_atomic::write_private_json(credentials_path, &creds)?;
    let notice = format!(
        "username={}\npassword={}\n",
        creds.username, initial_password
    );
    hq_fs_atomic::write_private_atomic_with_parent(initial_pw_path, notice.as_bytes())?;
    Ok(creds)
}

fn hash_argon2(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let params = Params::new(19 * 1024, 2, 1, None)
        .map_err(|e| anyhow::anyhow!("argon2 params error: {e}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| anyhow::anyhow!("argon2 hash error: {e}"))
}

fn verify_argon2(password: &str, encoded_hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(encoded_hash)
        .map_err(|e| anyhow::anyhow!("invalid password hash: {e}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
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

    struct TempAuthDir(PathBuf);

    impl TempAuthDir {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir()
                .join(format!("hq-auth-{label}-{}-{nonce}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempAuthDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn bootstrap_login_cookie_and_password_change_flow() {
        let dir = TempAuthDir::new("flow");
        let service = AuthService::initialize(&dir.0, AuthConfig::for_app("hqcode")).unwrap();
        assert!(service.has_initial_password_pending());

        let pw_file = fs::read_to_string(service.initial_password_path()).unwrap();
        let initial_pw = pw_file
            .lines()
            .find_map(|l| l.strip_prefix("password="))
            .unwrap()
            .to_string();

        assert!(!service.verify_login("hqcode", "wrong-pass").await.unwrap());
        assert!(service.has_initial_password_pending());

        assert!(service.verify_login("hqcode", &initial_pw).await.unwrap());
        assert!(!service.has_initial_password_pending());

        let token = service.issue_session_cookie("hqcode").unwrap();
        assert_eq!(
            service.verify_session_cookie(&token).as_deref(),
            Some("hqcode")
        );
        assert!(service.verify_session_cookie("tampered.sig").is_none());

        service
            .change_password(&initial_pw, Some("admin"), "new-strong-password")
            .await
            .unwrap();
        assert!(
            service
                .verify_login("admin", "new-strong-password")
                .await
                .unwrap()
        );
        assert!(service.verify_local_token(service.local_api_token()));
        assert!(!service.verify_local_token("bad-token"));
    }
}
