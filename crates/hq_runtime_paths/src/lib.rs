//! Dynamic OS user profile, home/shell resolution, default data/workspace paths,
//! and thread-safe local time utilities.

use std::{
    env,
    ffi::{CStr, CString, OsStr},
    path::PathBuf,
};

use anyhow::{Result, anyhow};

#[cfg(unix)]
use std::sync::Mutex;

pub mod local_time;

pub const DEFAULT_USER_NAME: &str = "root";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProfile {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
    pub shell: PathBuf,
}

/// Resolves the current process user's profile via passwd on Unix (or environment
/// variables as fallback on non-Unix targets).
pub fn resolve_current_user() -> Option<UserProfile> {
    current_uid_profile().or_else(|| {
        user_profile_from_env(
            env::var("USERNAME").ok(),
            env::var("USER").ok().or_else(|| env::var("LOGNAME").ok()),
            env_path("HOME"),
            env_path("USERPROFILE"),
            env_path("SHELL"),
            env_path("COMSPEC"),
        )
    })
}

/// Resolves an arbitrary OS user by login name using `getpwnam` on Unix.
pub fn resolve_user_by_name(user_name: &str) -> Result<UserProfile> {
    let normalized = normalize_user_name(user_name)?;
    named_user_profile(&normalized).ok_or_else(|| anyhow!("user `{normalized}` does not exist"))
}

/// Resolves the home directory for `user_name`, preferring `$HOME` when
/// `user_name` matches the current process user (matching CLI tools like `codex`
/// and `claude`), and falling back to passwd otherwise.
pub fn resolve_user_home_preferring_env(user_name: &str) -> Result<PathBuf> {
    let normalized = normalize_user_name(user_name)?;
    if let Some(current) = current_uid_profile()
        && current.name == normalized
        && let Some(env_home) = absolute_env_path("HOME")
    {
        return Ok(env_home);
    }
    Ok(resolve_user_by_name(&normalized)?.home)
}

/// Resolves the current user's home directory, preferring `$HOME` over passwd.
pub fn resolve_current_user_home_or_env() -> Option<PathBuf> {
    absolute_env_path("HOME").or_else(current_uid_home)
}

/// Resolves the default application data directory, honoring `env_var` if set
/// and otherwise placing `.{app_dir_name}` inside the user's home (or cwd).
pub fn resolve_data_dir(env_var: &str, app_dir_name: &str) -> PathBuf {
    if let Some(path) = env_path(env_var).filter(|p| !p.as_os_str().is_empty()) {
        return path;
    }
    if let Some(home) = resolve_current_user_home_or_env() {
        return home.join(app_dir_name);
    }
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(app_dir_name)
}

/// Resolves the default workspace directory, honoring `env_var` if set and
/// falling back to the current working directory or user's home.
pub fn resolve_workspace_dir(env_var: &str) -> PathBuf {
    if let Some(path) = env_path(env_var).filter(|p| !p.as_os_str().is_empty()) {
        return path;
    }
    env::current_dir()
        .ok()
        .or_else(resolve_current_user_home_or_env)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn list_login_user_profiles() -> Vec<UserProfile> {
    let mut profiles = all_user_profiles()
        .into_iter()
        .filter(is_login_user_profile)
        .collect::<Vec<_>>();
    profiles.sort_by(|left, right| {
        (left.name != DEFAULT_USER_NAME, left.uid, &left.name).cmp(&(
            right.name != DEFAULT_USER_NAME,
            right.uid,
            &right.name,
        ))
    });
    profiles.dedup_by(|left, right| left.name == right.name);
    profiles
}

pub fn normalize_user_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        anyhow::bail!("username cannot be empty");
    }
    if name.as_bytes().contains(&0) {
        anyhow::bail!("username cannot contain NUL bytes");
    }
    Ok(name.to_string())
}

fn absolute_env_path(name: &str) -> Option<PathBuf> {
    env_path(name).filter(|path| path.is_absolute())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name).map(PathBuf::from)
}

fn user_profile_from_env(
    username_env: Option<String>,
    user_env: Option<String>,
    home_env: Option<PathBuf>,
    userprofile_env: Option<PathBuf>,
    shell_env: Option<PathBuf>,
    comspec_env: Option<PathBuf>,
) -> Option<UserProfile> {
    let name = username_env
        .or(user_env)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let home = userprofile_env.or(home_env)?;
    let shell = shell_env
        .or(comspec_env)
        .unwrap_or_else(default_platform_shell);

    Some(UserProfile {
        name,
        uid: current_uid_value(),
        gid: current_gid_value(),
        home,
        shell,
    })
}

#[cfg(windows)]
fn default_platform_shell() -> PathBuf {
    PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe")
}

#[cfg(not(windows))]
fn default_platform_shell() -> PathBuf {
    PathBuf::from("/bin/sh")
}

fn is_login_user_profile(profile: &UserProfile) -> bool {
    if profile.name == DEFAULT_USER_NAME {
        return true;
    }
    if profile.uid < 1000 {
        return false;
    }
    let shell_name = profile
        .shell
        .file_name()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    !matches!(
        shell_name.as_str(),
        "nologin" | "false" | "sync" | "shutdown" | "halt"
    )
}

#[cfg(unix)]
fn current_uid_home() -> Option<PathBuf> {
    current_uid_profile().map(|profile| profile.home)
}

#[cfg(unix)]
fn current_uid_profile() -> Option<UserProfile> {
    with_passwd_database_lock(|| {
        let uid = unsafe { libc::geteuid() };
        let passwd = unsafe { libc::getpwuid(uid) };
        passwd_to_profile(passwd)
    })
}

#[cfg(unix)]
fn named_user_profile(name: &str) -> Option<UserProfile> {
    let name = CString::new(name).ok()?;
    with_passwd_database_lock(|| {
        let passwd = unsafe { libc::getpwnam(name.as_ptr()) };
        passwd_to_profile(passwd)
    })
}

#[cfg(unix)]
fn all_user_profiles() -> Vec<UserProfile> {
    with_passwd_database_lock(|| {
        let mut profiles = Vec::new();
        unsafe {
            libc::setpwent();
            loop {
                let passwd = libc::getpwent();
                if passwd.is_null() {
                    break;
                }
                if let Some(profile) = passwd_to_profile(passwd) {
                    profiles.push(profile);
                }
            }
            libc::endpwent();
        }
        profiles
    })
}

#[cfg(unix)]
fn with_passwd_database_lock<T>(operation: impl FnOnce() -> T) -> T {
    static PASSWD_DATABASE_LOCK: Mutex<()> = Mutex::new(());
    let _guard = PASSWD_DATABASE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    operation()
}

#[cfg(unix)]
fn passwd_to_profile(passwd: *mut libc::passwd) -> Option<UserProfile> {
    use std::os::unix::ffi::OsStrExt;

    if passwd.is_null() {
        return None;
    }
    let name_ptr = unsafe { (*passwd).pw_name };
    if name_ptr.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(name_ptr) }
        .to_string_lossy()
        .to_string();

    let home_ptr = unsafe { (*passwd).pw_dir };
    if home_ptr.is_null() {
        return None;
    }
    let home_bytes = unsafe { CStr::from_ptr(home_ptr) }.to_bytes();
    let home = PathBuf::from(OsStr::from_bytes(home_bytes));
    if !home.is_absolute() {
        return None;
    }

    let shell_ptr = unsafe { (*passwd).pw_shell };
    let shell = if shell_ptr.is_null() {
        PathBuf::from("/bin/sh")
    } else {
        let shell_bytes = unsafe { CStr::from_ptr(shell_ptr) }.to_bytes();
        let shell = PathBuf::from(OsStr::from_bytes(shell_bytes));
        if shell.is_absolute() {
            shell
        } else {
            PathBuf::from("/bin/sh")
        }
    };

    Some(UserProfile {
        name,
        uid: unsafe { (*passwd).pw_uid },
        gid: unsafe { (*passwd).pw_gid },
        home,
        shell,
    })
}

#[cfg(unix)]
fn current_uid_value() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn current_gid_value() -> u32 {
    unsafe { libc::getegid() }
}

#[cfg(not(unix))]
fn current_uid_home() -> Option<PathBuf> {
    resolve_current_user().map(|profile| profile.home)
}

#[cfg(not(unix))]
fn current_uid_profile() -> Option<UserProfile> {
    user_profile_from_env(
        env::var("USERNAME").ok(),
        env::var("USER").ok().or_else(|| env::var("LOGNAME").ok()),
        env_path("HOME"),
        env_path("USERPROFILE"),
        env_path("SHELL"),
        env_path("COMSPEC"),
    )
}

#[cfg(not(unix))]
fn named_user_profile(name: &str) -> Option<UserProfile> {
    let current = current_uid_profile()?;
    if current.name.eq_ignore_ascii_case(name) {
        Some(current)
    } else {
        None
    }
}

#[cfg(not(unix))]
fn all_user_profiles() -> Vec<UserProfile> {
    current_uid_profile().into_iter().collect()
}

#[cfg(not(unix))]
fn current_uid_value() -> u32 {
    0
}

#[cfg(not(unix))]
fn current_gid_value() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_user_resolves_with_valid_home_and_shell() {
        let user = resolve_current_user().expect("current user must resolve");
        assert!(!user.name.is_empty());
        assert!(user.home.is_absolute());
        assert!(user.shell.is_absolute());
    }

    #[cfg(unix)]
    #[test]
    fn nonexistent_user_returns_error() {
        assert!(resolve_user_by_name("hqcode_nonexistent_user_98765").is_err());
    }
}
