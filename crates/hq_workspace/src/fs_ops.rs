use std::{
    cmp::Ordering,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tokio::fs;

const MAX_EDITABLE_FILE_SIZE: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
    pub display_root: PathBuf,
    pub show_dot_entries: bool,
    pub allow_one_parent: bool,
}

impl WorkspaceConfig {
    pub fn new(root: PathBuf) -> Self {
        let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        Self {
            display_root: root,
            root: canonical,
            show_dot_entries: true,
            allow_one_parent: true,
        }
    }

    fn allowed_root(&self) -> PathBuf {
        if self.allow_one_parent {
            self.root
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone())
        } else {
            self.root.clone()
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DirectoryResponse {
    pub path: String,
    pub display_path: String,
    pub parent_path: Option<String>,
    pub entries: Vec<DirectoryEntry>,
}

#[derive(Debug, Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
    pub kind: &'static str,
    pub symlink: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct FileResponse {
    pub path: String,
    pub display_path: String,
    pub editable: bool,
    pub size: u64,
    pub content: String,
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SaveFileResponse {
    pub ok: bool,
    pub saved_bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct RenamePathResponse {
    pub ok: bool,
    pub old_path: String,
    pub path: String,
    pub display_path: String,
    pub kind: &'static str,
}

/// Resolves a relative query path (supporting at most one `..` above `config.root`
/// when `allow_one_parent` is enabled) and guarantees the canonical target stays
/// inside the allowed boundary.
pub fn resolve_directory_path(config: &WorkspaceConfig, relative: &str) -> Result<PathBuf> {
    let target = resolve_candidate_path(&config.root, relative, config.allow_one_parent)?;
    let canonical = target
        .canonicalize()
        .with_context(|| format!("directory does not exist: {}", target.display()))?;
    ensure_within_boundary(&config.allowed_root(), &canonical)?;
    if !canonical.is_dir() {
        bail!("path is not a directory");
    }
    Ok(canonical)
}

pub fn resolve_existing_file_path(config: &WorkspaceConfig, relative: &str) -> Result<PathBuf> {
    let target = resolve_candidate_path(&config.root, relative, config.allow_one_parent)?;
    let canonical = target
        .canonicalize()
        .with_context(|| format!("file does not exist: {}", target.display()))?;
    ensure_within_boundary(&config.allowed_root(), &canonical)?;
    if !canonical.is_file() {
        bail!("path is not a regular file");
    }
    Ok(canonical)
}

fn resolve_candidate_path(
    base_root: &Path,
    relative: &str,
    allow_one_parent: bool,
) -> Result<PathBuf> {
    let trimmed = relative.trim();
    if trimmed.is_empty() || trimmed == "." {
        return Ok(base_root.to_path_buf());
    }

    let rel_path = Path::new(trimmed);
    if rel_path.is_absolute() {
        // Allow absolute path only if it resolves within boundary.
        let allowed = if allow_one_parent {
            base_root
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| base_root.to_path_buf())
        } else {
            base_root.to_path_buf()
        };
        let canonical = rel_path
            .canonicalize()
            .with_context(|| format!("path does not exist: {trimmed}"))?;
        ensure_within_boundary(&allowed, &canonical)?;
        return Ok(canonical);
    }

    let mut depth: i32 = 0;
    let min_depth = if allow_one_parent { -1 } else { 0 };
    for component in rel_path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(_) => depth += 1,
            Component::ParentDir => {
                depth -= 1;
                if depth < min_depth {
                    bail!("path escapes allowed workspace boundary");
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!("invalid path component");
            }
        }
    }

    Ok(base_root.join(rel_path))
}

fn ensure_within_boundary(boundary: &Path, target: &Path) -> Result<()> {
    let canonical_boundary = boundary
        .canonicalize()
        .unwrap_or_else(|_| boundary.to_path_buf());
    if !target.starts_with(&canonical_boundary) {
        bail!("access denied: path outside allowed workspace");
    }
    Ok(())
}

fn relative_for_boundary(config: &WorkspaceConfig, target: &Path) -> String {
    if target == config.root {
        return String::new();
    }
    if let Ok(rel) = target.strip_prefix(&config.root) {
        return rel.to_string_lossy().replace('\\', "/");
    }
    if config.allow_one_parent
        && let Some(parent) = config.root.parent()
        && target == parent
    {
        return "..".to_string();
    }
    if config.allow_one_parent
        && let Some(parent) = config.root.parent()
        && let Ok(rel) = target.strip_prefix(parent)
    {
        return format!("../{}", rel.to_string_lossy().replace('\\', "/"));
    }
    target.to_string_lossy().to_string()
}

fn display_path_for(config: &WorkspaceConfig, canonical: &Path) -> String {
    if let Ok(rel) = canonical.strip_prefix(&config.root) {
        if rel.as_os_str().is_empty() {
            return config.display_root.to_string_lossy().into_owned();
        }
        return config.display_root.join(rel).to_string_lossy().into_owned();
    }
    canonical.to_string_lossy().into_owned()
}

pub async fn list_entries(config: &WorkspaceConfig, relative: &str) -> Result<DirectoryResponse> {
    let directory = resolve_directory_path(config, relative)?;
    let rel = relative_for_boundary(config, &directory);
    let display_path = display_path_for(config, &directory);
    let parent_path = if directory == config.allowed_root() {
        None
    } else {
        directory
            .parent()
            .filter(|p| p.starts_with(config.allowed_root()))
            .map(|p| relative_for_boundary(config, p))
    };

    let mut reader = fs::read_dir(&directory)
        .await
        .with_context(|| format!("failed to read directory {}", directory.display()))?;
    let mut entries = Vec::new();

    while let Some(entry) = reader
        .next_entry()
        .await
        .context("failed to read directory entry")?
    {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().into_owned();
        if !config.show_dot_entries && name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        let symlink = file_type.is_symlink();
        let meta = fs::metadata(entry.path()).await.ok();
        let is_dir = meta
            .as_ref()
            .map(|m| m.is_dir())
            .unwrap_or(file_type.is_dir());
        let kind = if is_dir { "dir" } else { "file" };
        let size = if is_dir {
            None
        } else {
            meta.as_ref().map(|m| m.len())
        };
        let entry_rel = if rel.is_empty() {
            name.clone()
        } else if rel == ".." {
            format!("../{name}")
        } else {
            format!("{rel}/{name}")
        };
        entries.push(DirectoryEntry {
            name,
            path: entry_rel,
            kind,
            symlink,
            size,
        });
    }

    entries.sort_by(|a, b| match (a.kind, b.kind) {
        ("dir", "file") => Ordering::Less,
        ("file", "dir") => Ordering::Greater,
        _ => a
            .name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.name.cmp(&b.name)),
    });

    Ok(DirectoryResponse {
        path: rel,
        display_path,
        parent_path,
        entries,
    })
}

pub async fn read_utf8_file(config: &WorkspaceConfig, relative: &str) -> Result<FileResponse> {
    let path = resolve_existing_file_path(config, relative)?;
    let metadata = fs::metadata(&path)
        .await
        .with_context(|| format!("failed to stat {}", path.display()))?;
    let size = metadata.len();
    let rel = relative_for_boundary(config, &path);
    let display_path = display_path_for(config, &path);

    if size > MAX_EDITABLE_FILE_SIZE {
        return Ok(FileResponse {
            path: rel,
            display_path,
            editable: false,
            size,
            content: String::new(),
            message: Some(format!(
                "File is too large for inline editing ({size} bytes; limit is {MAX_EDITABLE_FILE_SIZE} bytes)."
            )),
        });
    }

    let bytes = fs::read(&path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    match String::from_utf8(bytes) {
        Ok(content) => Ok(FileResponse {
            path: rel,
            display_path,
            editable: true,
            size,
            content,
            message: None,
        }),
        Err(_) => Ok(FileResponse {
            path: rel,
            display_path,
            editable: false,
            size,
            content: String::new(),
            message: Some("Binary or non-UTF-8 files cannot be edited inline.".to_string()),
        }),
    }
}

pub async fn write_utf8_file(
    config: &WorkspaceConfig,
    relative: &str,
    content: &str,
) -> Result<SaveFileResponse> {
    let target = resolve_candidate_path(&config.root, relative, config.allow_one_parent)?;
    let parent = target
        .parent()
        .context("target file must have a parent directory")?;
    let canonical_parent = parent
        .canonicalize()
        .with_context(|| format!("parent directory does not exist: {}", parent.display()))?;
    ensure_within_boundary(&config.allowed_root(), &canonical_parent)?;

    if let Ok(meta) = fs::symlink_metadata(&target).await
        && meta.file_type().is_symlink()
    {
        let canonical = target.canonicalize()?;
        ensure_within_boundary(&config.allowed_root(), &canonical)?;
    }

    let bytes = content.as_bytes().to_vec();
    let saved_bytes = bytes.len();
    let target_clone = target.clone();
    tokio::task::spawn_blocking(move || hq_fs_atomic::write_atomic(&target_clone, &bytes))
        .await
        .context("atomic save task panicked")?
        .with_context(|| format!("failed to write {}", target.display()))?;

    Ok(SaveFileResponse {
        ok: true,
        saved_bytes,
    })
}

pub async fn rename_entry(
    config: &WorkspaceConfig,
    relative: &str,
    new_name: &str,
) -> Result<RenamePathResponse> {
    let trimmed_name = new_name.trim();
    if trimmed_name.is_empty()
        || trimmed_name == "."
        || trimmed_name == ".."
        || trimmed_name.contains('/')
        || trimmed_name.contains('\\')
        || trimmed_name.as_bytes().contains(&0)
    {
        bail!("invalid destination filename");
    }

    let candidate = resolve_candidate_path(&config.root, relative, config.allow_one_parent)?;
    let canonical_src = candidate
        .canonicalize()
        .with_context(|| format!("source path does not exist: {}", candidate.display()))?;
    ensure_within_boundary(&config.allowed_root(), &canonical_src)?;
    if canonical_src == config.root || canonical_src == config.allowed_root() {
        bail!("cannot rename workspace root");
    }

    let parent = canonical_src
        .parent()
        .context("source path must have a parent directory")?;
    let destination = parent.join(trimmed_name);
    match fs::symlink_metadata(&destination).await {
        Ok(_) => bail!("destination already exists"),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("failed to inspect destination"),
    }

    let old_rel = relative_for_boundary(config, &canonical_src);
    let kind = if canonical_src.is_dir() {
        "dir"
    } else {
        "file"
    };
    fs::rename(&canonical_src, &destination)
        .await
        .context("failed to rename entry")?;
    let canonical_dst = destination.canonicalize().unwrap_or(destination);

    Ok(RenamePathResponse {
        ok: true,
        old_path: old_rel,
        path: relative_for_boundary(config, &canonical_dst),
        display_path: display_path_for(config, &canonical_dst),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempWorkspace(PathBuf);

    impl TempWorkspace {
        fn new(label: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root =
                std::env::temp_dir().join(format!("hq-ws-{label}-{}-{nonce}", std::process::id()));
            std::fs::create_dir_all(root.join("project/sub")).unwrap();
            std::fs::write(root.join("project/hello.txt"), "world").unwrap();
            std::fs::write(root.join("outside.txt"), "secret").unwrap();
            Self(root)
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn allows_one_parent_and_blocks_deeper_traversal() {
        let tmp = TempWorkspace::new("boundary");
        let cfg = WorkspaceConfig::new(tmp.0.join("project"));

        let list = list_entries(&cfg, "").await.unwrap();
        assert!(list.entries.iter().any(|e| e.name == "hello.txt"));

        let parent_list = list_entries(&cfg, "..").await.unwrap();
        assert!(parent_list.entries.iter().any(|e| e.name == "outside.txt"));

        let err = list_entries(&cfg, "../..").await.unwrap_err();
        assert!(err.to_string().contains("escapes"));
    }

    #[tokio::test]
    async fn read_write_and_rename_roundtrip() {
        let tmp = TempWorkspace::new("rw");
        let cfg = WorkspaceConfig::new(tmp.0.join("project"));

        write_utf8_file(&cfg, "sub/note.md", "hi hqcode")
            .await
            .unwrap();
        let file = read_utf8_file(&cfg, "sub/note.md").await.unwrap();
        assert!(file.editable);
        assert_eq!(file.content, "hi hqcode");

        let renamed = rename_entry(&cfg, "sub/note.md", "note2.md").await.unwrap();
        assert_eq!(renamed.path, "sub/note2.md");
        let file2 = read_utf8_file(&cfg, "sub/note2.md").await.unwrap();
        assert_eq!(file2.content, "hi hqcode");
    }
}
