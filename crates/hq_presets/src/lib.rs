//! Installed Agent CLI detection and real-config-file preset switching
//! (permanent `apply_preset` and temporary transaction `run_with_temporary_preset`)
//! without injecting CODEX_HOME or CLAUDE_CONFIG_DIR bypass variables.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use hq_runtime_paths::UserProfile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PresetKind {
    CodexApi,
    ClaudeApi,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPreset {
    pub id: String,
    pub name: String,
    pub kind: PresetKind,
    pub model: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub wire_api: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedAgent {
    pub id: String,
    pub label: String,
    pub command: String,
    pub path: String,
    pub available: bool,
}

pub fn detect_installed_agents() -> Vec<DetectedAgent> {
    let candidates = [
        ("claude", "Claude Code", "claude"),
        ("codex", "Codex CLI", "codex"),
        ("opencode", "OpenCode", "opencode"),
        ("shell", "Interactive Shell", "bash"),
    ];
    candidates
        .into_iter()
        .map(|(id, label, cmd)| {
            let resolved = find_in_path(cmd);
            DetectedAgent {
                id: id.to_string(),
                label: label.to_string(),
                command: cmd.to_string(),
                path: resolved
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                available: resolved.is_some(),
            }
        })
        .collect()
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let full = dir.join(binary);
        if full.is_file() {
            return Some(full);
        }
    }
    None
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedPresets {
    presets: Vec<ModelPreset>,
}

#[derive(Clone)]
pub struct PresetStore {
    path: PathBuf,
    presets: Arc<RwLock<Vec<ModelPreset>>>,
    write_lock: Arc<Mutex<()>>,
}

impl PresetStore {
    pub fn load(data_dir: &Path, prefix: &str) -> Self {
        let path = data_dir.join(format!("{prefix}-presets.json"));
        let presets = hq_fs_atomic::read_private_json::<PersistedPresets>(&path)
            .map(|p| p.presets)
            .unwrap_or_default();
        Self {
            path,
            presets: Arc::new(RwLock::new(presets)),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn list(&self) -> Vec<ModelPreset> {
        self.presets
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn upsert(&self, mut preset: ModelPreset) -> Result<ModelPreset> {
        let name = preset.name.trim().to_string();
        if name.is_empty() {
            bail!("preset name cannot be empty");
        }
        preset.name = name;
        if preset.id.trim().is_empty() {
            preset.id = format!(
                "p-{}",
                preset
                    .name
                    .to_ascii_lowercase()
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect::<String>()
            );
        }

        let _guard = self.write_lock.lock().unwrap_or_else(|p| p.into_inner());
        {
            let mut list = self.presets.write().unwrap_or_else(|p| p.into_inner());
            if let Some(existing) = list.iter_mut().find(|p| p.id == preset.id) {
                *existing = preset.clone();
            } else {
                list.push(preset.clone());
            }
            let payload = PersistedPresets {
                presets: list.clone(),
            };
            hq_fs_atomic::write_private_json(&self.path, &payload)?;
        }
        Ok(preset)
    }

    pub fn remove(&self, id: &str) -> Result<bool> {
        let _guard = self.write_lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut list = self.presets.write().unwrap_or_else(|p| p.into_inner());
        let before = list.len();
        list.retain(|p| p.id != id);
        let removed = list.len() != before;
        if removed {
            let payload = PersistedPresets {
                presets: list.clone(),
            };
            hq_fs_atomic::write_private_json(&self.path, &payload)?;
        }
        Ok(removed)
    }

    pub fn resolve(&self, selector: &str) -> Option<ModelPreset> {
        let list = self.list();
        if let Some(by_id) = list.iter().find(|p| p.id == selector) {
            return Some(by_id.clone());
        }
        let by_name: Vec<_> = list.iter().filter(|p| p.name == selector).collect();
        if by_name.len() == 1 {
            return Some(by_name[0].clone());
        }
        list.into_iter().find(|p| p.model == selector)
    }
}

#[derive(Clone)]
struct FileBackup {
    path: PathBuf,
    original: Option<Vec<u8>>,
}

impl FileBackup {
    fn capture(path: PathBuf) -> Result<Self> {
        let original = if path.exists() {
            Some(fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?)
        } else {
            None
        };
        Ok(Self { path, original })
    }

    fn restore(&self) -> Result<()> {
        match &self.original {
            Some(bytes) => hq_fs_atomic::write_private_atomic_with_parent(&self.path, bytes)?,
            None => {
                if self.path.exists() {
                    let _ = fs::remove_file(&self.path);
                }
            }
        }
        Ok(())
    }
}

pub fn apply_preset(user_home: &Path, preset: &ModelPreset) -> Result<()> {
    match preset.kind {
        PresetKind::CodexApi => apply_codex_preset(user_home, preset),
        PresetKind::ClaudeApi => apply_claude_preset(user_home, preset),
    }
}

fn apply_codex_preset(user_home: &Path, preset: &ModelPreset) -> Result<()> {
    let codex_dir = user_home.join(".codex");
    fs::create_dir_all(&codex_dir)?;
    let config_path = codex_dir.join("config.toml");
    let raw_toml = if config_path.exists() {
        fs::read_to_string(&config_path)?
    } else {
        String::new()
    };
    let mut doc = raw_toml
        .parse::<toml_edit::DocumentMut>()
        .unwrap_or_default();

    let provider_id = "hqcode_provider";
    doc["model_provider"] = toml_edit::value(provider_id);
    if !preset.model.trim().is_empty() {
        doc["model"] = toml_edit::value(preset.model.trim());
    }
    let providers = doc
        .entry("model_providers")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
    if let Some(table) = providers.as_table_mut() {
        let entry = table
            .entry(provider_id)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
        if let Some(p) = entry.as_table_mut() {
            p["name"] = toml_edit::value(preset.name.as_str());
            if !preset.base_url.trim().is_empty() {
                p["base_url"] = toml_edit::value(preset.base_url.trim());
            }
            let wire = if preset.wire_api.trim().is_empty() {
                "responses"
            } else {
                preset.wire_api.trim()
            };
            p["wire_api"] = toml_edit::value(wire);
        }
    }
    hq_fs_atomic::write_atomic(&config_path, doc.to_string().as_bytes())?;

    if !preset.api_key.trim().is_empty() {
        let auth_path = codex_dir.join("auth.json");
        let mut auth_json: serde_json::Value = if auth_path.exists() {
            fs::read(&auth_path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_else(|| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };
        if let Some(obj) = auth_json.as_object_mut() {
            obj.insert(
                "OPENAI_API_KEY".to_string(),
                serde_json::Value::String(preset.api_key.trim().to_string()),
            );
        }
        hq_fs_atomic::write_private_json(&auth_path, &auth_json)?;
    }

    Ok(())
}

fn apply_claude_preset(user_home: &Path, preset: &ModelPreset) -> Result<()> {
    let claude_dir = user_home.join(".claude");
    fs::create_dir_all(&claude_dir)?;
    let settings_path = claude_dir.join("settings.json");
    let mut root: serde_json::Value = if settings_path.exists() {
        fs::read(&settings_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let env_obj = root
        .as_object_mut()
        .context("settings.json root must be a JSON object")?
        .entry("env")
        .or_insert_with(|| serde_json::json!({}));
    let map = env_obj
        .as_object_mut()
        .context("settings.json env must be a JSON object")?;
    if !preset.api_key.trim().is_empty() {
        map.insert(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            serde_json::Value::String(preset.api_key.trim().to_string()),
        );
    }
    if !preset.base_url.trim().is_empty() {
        map.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            serde_json::Value::String(preset.base_url.trim().to_string()),
        );
    }
    if !preset.model.trim().is_empty() {
        map.insert(
            "ANTHROPIC_MODEL".to_string(),
            serde_json::Value::String(preset.model.trim().to_string()),
        );
    }
    hq_fs_atomic::write_private_json(&settings_path, &root)?;
    Ok(())
}

/// Applies `preset` to the real shared configuration under `user_home`, spawns
/// `cmd_args` natively (never injecting `CODEX_HOME` or `CLAUDE_CONFIG_DIR`),
/// restores the original configuration after `settle_delay` (or when the child
/// exits earlier), and returns the child's exit code.
pub fn run_with_temporary_preset(
    _user_profile: &UserProfile,
    user_home: &Path,
    preset: &ModelPreset,
    cmd_args: &[String],
    settle_delay: Duration,
) -> Result<i32> {
    if cmd_args.is_empty() {
        bail!("missing command to execute");
    }

    let backups = match preset.kind {
        PresetKind::CodexApi => vec![
            FileBackup::capture(user_home.join(".codex/config.toml"))?,
            FileBackup::capture(user_home.join(".codex/auth.json"))?,
        ],
        PresetKind::ClaudeApi => vec![FileBackup::capture(
            user_home.join(".claude/settings.json"),
        )?],
    };

    apply_preset(user_home, preset)?;

    let mut command = Command::new(&cmd_args[0]);
    if cmd_args.len() > 1 {
        command.args(&cmd_args[1..]);
    }
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(err) => {
            for b in &backups {
                let _ = b.restore();
            }
            return Err(err).with_context(|| format!("failed to spawn {}", cmd_args[0]));
        }
    };

    let restored = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let restored_bg = restored.clone();
    let backups_bg = backups.clone();
    std::thread::spawn(move || {
        std::thread::sleep(settle_delay);
        if !restored_bg.swap(true, std::sync::atomic::Ordering::SeqCst) {
            for b in &backups_bg {
                let _ = b.restore();
            }
        }
    });

    let status = child.wait().context("failed waiting for launched agent")?;
    if !restored.swap(true, std::sync::atomic::Ordering::SeqCst) {
        for b in &backups {
            let _ = b.restore();
        }
    }
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempHome(PathBuf);

    impl TempHome {
        fn new(label: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir()
                .join(format!("hq-presets-{label}-{}-{nonce}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn permanent_and_temporary_preset_transactions() {
        let home = TempHome::new("tx");
        let store = PresetStore::load(&home.0, "hqcode");
        let original = ModelPreset {
            id: "orig".to_string(),
            name: "Original".to_string(),
            kind: PresetKind::CodexApi,
            model: "gpt-5".to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-orig".to_string(),
            wire_api: "responses".to_string(),
        };
        let temp = ModelPreset {
            id: "temp".to_string(),
            name: "Temp".to_string(),
            kind: PresetKind::CodexApi,
            model: "gpt-5-mini".to_string(),
            base_url: "https://temp.example.com/v1".to_string(),
            api_key: "sk-temp".to_string(),
            wire_api: "responses".to_string(),
        };
        store.upsert(original.clone()).unwrap();
        store.upsert(temp.clone()).unwrap();

        apply_preset(&home.0, &original).unwrap();
        let before_toml = fs::read_to_string(home.0.join(".codex/config.toml")).unwrap();
        assert!(before_toml.contains("gpt-5"));

        let profile = hq_runtime_paths::resolve_current_user().unwrap();
        let code = run_with_temporary_preset(
            &profile,
            &home.0,
            &temp,
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                format!(
                    "grep -q 'gpt-5-mini' {}/.codex/config.toml",
                    home.0.display()
                ),
            ],
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(code, 0);

        let after_toml = fs::read_to_string(home.0.join(".codex/config.toml")).unwrap();
        assert_eq!(after_toml, before_toml);
    }

    #[test]
    fn detect_installed_agents_includes_shell() {
        let agents = detect_installed_agents();
        assert!(agents.iter().any(|a| a.id == "shell" && a.available));
    }
}
