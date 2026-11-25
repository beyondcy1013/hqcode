use std::{
    path::{Component, Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tokio::{fs, process::Command, time::timeout};

const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DIFF_BYTES: usize = 512 * 1024;
const MAX_UNTRACKED_PREVIEW_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GitChangedFile {
    pub path: String,
    pub status: String,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GitStatusSummary {
    pub is_repo: bool,
    pub repo_root: Option<String>,
    pub branch: String,
    pub files: Vec<GitChangedFile>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GitDiffResponse {
    pub is_repo: bool,
    pub file: Option<String>,
    pub diff: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GitCommitResult {
    pub commit: String,
    pub summary: String,
}

async fn run_git(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir).args(args).kill_on_drop(true);
    let output = timeout(GIT_COMMAND_TIMEOUT, cmd.output())
        .await
        .context("git command timed out")?
        .context("failed to execute git")?;
    Ok(output)
}

async fn resolve_repo_root(dir: &Path) -> Result<Option<PathBuf>> {
    if !dir.exists() {
        return Ok(None);
    }
    let output = run_git(dir, &["rev-parse", "--show-toplevel"]).await?;
    if !output.status.success() {
        return Ok(None);
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(root)))
}

fn validate_repo_relative_path(raw: &str) -> Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("file path cannot be empty");
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        bail!("file path must be relative to repository root");
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("invalid repository-relative file path");
            }
        }
    }
    Ok(path.to_path_buf())
}

fn decode_git_porcelain_line(line: &str) -> Option<GitChangedFile> {
    if line.len() < 4 {
        return None;
    }
    let bytes = line.as_bytes();
    let x = bytes[0] as char;
    let y = bytes[1] as char;
    let rest = line[3..].trim();
    if rest.is_empty() {
        return None;
    }
    let path = if let Some((_, new_path)) = rest.split_once(" -> ") {
        new_path.trim().trim_matches('"').to_string()
    } else {
        rest.trim_matches('"').to_string()
    };

    let untracked = x == '?' && y == '?';
    let staged = !untracked && x != ' ';
    let unstaged = !untracked && y != ' ';
    let status = if untracked {
        "??".to_string()
    } else {
        format!("{x}{y}")
    };

    Some(GitChangedFile {
        path,
        status,
        staged,
        unstaged,
        untracked,
    })
}

pub async fn git_status(dir: &Path) -> Result<GitStatusSummary> {
    let Some(repo_root) = resolve_repo_root(dir).await? else {
        return Ok(GitStatusSummary {
            is_repo: false,
            repo_root: None,
            branch: String::new(),
            files: Vec::new(),
        });
    };

    let branch_out = run_git(&repo_root, &["symbolic-ref", "--short", "HEAD"]).await?;
    let branch = if branch_out.status.success() {
        String::from_utf8_lossy(&branch_out.stdout).trim().to_string()
    } else {
        let rev_out = run_git(&repo_root, &["rev-parse", "--short", "HEAD"]).await?;
        if rev_out.status.success() {
            format!("detached@{}", String::from_utf8_lossy(&rev_out.stdout).trim())
        } else {
            "HEAD (unborn)".to_string()
        }
    };

    let status_out = run_git(&repo_root, &["status", "--porcelain=v1", "-uall"]).await?;
    if !status_out.status.success() {
        bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&status_out.stderr).trim()
        );
    }

    let files = String::from_utf8_lossy(&status_out.stdout)
        .lines()
        .filter_map(decode_git_porcelain_line)
        .collect();

    Ok(GitStatusSummary {
        is_repo: true,
        repo_root: Some(repo_root.to_string_lossy().into_owned()),
        branch,
        files,
    })
}

pub async fn git_diff(dir: &Path, file_path: Option<&str>) -> Result<GitDiffResponse> {
    let Some(repo_root) = resolve_repo_root(dir).await? else {
        return Ok(GitDiffResponse {
            is_repo: false,
            file: file_path.map(str::to_string),
            diff: String::new(),
            truncated: false,
        });
    };

    let rel_file = match file_path.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => Some(validate_repo_relative_path(raw)?),
        None => None,
    };

    let has_head = run_git(&repo_root, &["rev-parse", "--verify", "HEAD"])
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    let mut diff_text = String::new();
    if let Some(ref rel) = rel_file {
        let rel_str = rel.to_string_lossy().into_owned();
        let out = if has_head {
            run_git(&repo_root, &["diff", "HEAD", "--", &rel_str]).await?
        } else {
            run_git(&repo_root, &["diff", "--", &rel_str]).await?
        };
        if out.status.success() {
            diff_text.push_str(&String::from_utf8_lossy(&out.stdout));
        }
        // If empty, also check staged diff when unborn or untracked file preview.
        if diff_text.trim().is_empty() {
            let cached = run_git(&repo_root, &["diff", "--cached", "--", &rel_str]).await?;
            if cached.status.success() {
                diff_text.push_str(&String::from_utf8_lossy(&cached.stdout));
            }
        }
        if diff_text.trim().is_empty() {
            diff_text = build_untracked_preview(&repo_root, rel).await?;
        }
    } else {
        let out = if has_head {
            run_git(&repo_root, &["diff", "HEAD"]).await?
        } else {
            run_git(&repo_root, &["diff"]).await?
        };
        if out.status.success() {
            diff_text.push_str(&String::from_utf8_lossy(&out.stdout));
        }
        if !has_head {
            let cached = run_git(&repo_root, &["diff", "--cached"]).await?;
            if cached.status.success() {
                diff_text.push_str(&String::from_utf8_lossy(&cached.stdout));
            }
        }
        let status = git_status(&repo_root).await?;
        for item in status.files.into_iter().filter(|f| f.untracked).take(10) {
            if diff_text.len() >= MAX_DIFF_BYTES {
                break;
            }
            if let Ok(preview) = build_untracked_preview(&repo_root, Path::new(&item.path)).await
                && !preview.is_empty()
            {
                if !diff_text.is_empty() && !diff_text.ends_with('\n') {
                    diff_text.push('\n');
                }
                diff_text.push_str(&preview);
            }
        }
    }

    let mut truncated = false;
    if diff_text.len() > MAX_DIFF_BYTES {
        let mut end = MAX_DIFF_BYTES;
        while !diff_text.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        diff_text.truncate(end);
        diff_text.push_str("\n... [diff truncated]\n");
        truncated = true;
    }

    Ok(GitDiffResponse {
        is_repo: true,
        file: rel_file.map(|p| p.to_string_lossy().into_owned()),
        diff: diff_text,
        truncated,
    })
}

async fn build_untracked_preview(repo_root: &Path, rel: &Path) -> Result<String> {
    let full = repo_root.join(rel);
    let Ok(meta) = fs::symlink_metadata(&full).await else {
        return Ok(String::new());
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Ok(String::new());
    }
    let rel_str = rel.to_string_lossy();
    if meta.len() as usize > MAX_UNTRACKED_PREVIEW_BYTES {
        return Ok(format!(
            "diff --git a/{rel_str} b/{rel_str}\nnew file (preview omitted: {} bytes)\n",
            meta.len()
        ));
    }
    let bytes = fs::read(&full).await?;
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(format!(
            "diff --git a/{rel_str} b/{rel_str}\nBinary file {rel_str} added\n"
        ));
    };
    let mut out = format!(
        "diff --git a/{rel_str} b/{rel_str}\nnew file mode 100644\n--- /dev/null\n+++ b/{rel_str}\n"
    );
    let lines: Vec<&str> = text.lines().collect();
    out.push_str(&format!("@@ -0,0 +1,{} @@\n", lines.len()));
    for line in lines {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

pub async fn git_revert_file(dir: &Path, file_path: &str) -> Result<()> {
    let repo_root = resolve_repo_root(dir)
        .await?
        .context("directory is not inside a git repository")?;
    let rel = validate_repo_relative_path(file_path)?;
    let rel_str = rel.to_string_lossy().into_owned();

    let ls_out = run_git(&repo_root, &["ls-files", "--error-unmatch", "--", &rel_str]).await?;
    if ls_out.status.success() {
        let has_head = run_git(&repo_root, &["rev-parse", "--verify", "HEAD"])
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if has_head {
            let _ = run_git(&repo_root, &["reset", "HEAD", "--", &rel_str]).await?;
            let checkout = run_git(&repo_root, &["checkout", "--", &rel_str]).await?;
            if !checkout.status.success() {
                bail!(
                    "git checkout failed: {}",
                    String::from_utf8_lossy(&checkout.stderr).trim()
                );
            }
        } else {
            let _ = run_git(&repo_root, &["rm", "--cached", "-f", "--", &rel_str]).await?;
        }
        return Ok(());
    }

    // Untracked file inside repository: verify canonical path stays inside repo_root.
    let target = repo_root.join(&rel);
    if let Ok(meta) = fs::symlink_metadata(&target).await {
        if meta.file_type().is_symlink() {
            fs::remove_file(&target)
                .await
                .with_context(|| format!("failed to remove symlink {}", target.display()))?;
            return Ok(());
        }
        let canonical_repo = repo_root.canonicalize()?;
        let canonical_target = target.canonicalize()?;
        if !canonical_target.starts_with(&canonical_repo) {
            bail!("refusing to delete file outside repository");
        }
        if canonical_target.is_file() {
            fs::remove_file(&canonical_target)
                .await
                .with_context(|| format!("failed to delete {}", canonical_target.display()))?;
        }
    }
    Ok(())
}

pub async fn git_commit(
    dir: &Path,
    message: &str,
    files: Option<Vec<String>>,
) -> Result<GitCommitResult> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        bail!("commit message cannot be empty");
    }
    let repo_root = resolve_repo_root(dir)
        .await?
        .context("directory is not inside a git repository")?;

    match files {
        Some(list) if !list.is_empty() => {
            let mut validated = Vec::with_capacity(list.len());
            for item in list {
                let rel = validate_repo_relative_path(&item)?;
                validated.push(rel.to_string_lossy().into_owned());
            }
            let mut args = vec!["add", "-A", "--"];
            for item in &validated {
                args.push(item.as_str());
            }
            let add_out = run_git(&repo_root, &args).await?;
            if !add_out.status.success() {
                bail!(
                    "git add failed: {}",
                    String::from_utf8_lossy(&add_out.stderr).trim()
                );
            }
        }
        _ => {
            let add_out = run_git(&repo_root, &["add", "-A"]).await?;
            if !add_out.status.success() {
                bail!(
                    "git add failed: {}",
                    String::from_utf8_lossy(&add_out.stderr).trim()
                );
            }
        }
    }

    let commit_out = run_git(&repo_root, &["commit", "-m", trimmed]).await?;
    if !commit_out.status.success() {
        let stderr = String::from_utf8_lossy(&commit_out.stderr);
        let stdout = String::from_utf8_lossy(&commit_out.stdout);
        bail!("git commit failed: {} {}", stdout.trim(), stderr.trim());
    }

    let sha_out = run_git(&repo_root, &["rev-parse", "--short", "HEAD"]).await?;
    let commit = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();
    let summary = String::from_utf8_lossy(&commit_out.stdout)
        .lines()
        .next()
        .unwrap_or("committed")
        .trim()
        .to_string();

    Ok(GitCommitResult { commit, summary })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempGitRepo(PathBuf);

    impl TempGitRepo {
        async fn new(label: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "hq-git-{label}-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            assert!(run_git(&dir, &["init"]).await.unwrap().status.success());
            assert!(
                run_git(&dir, &["config", "user.name", "HQCode Test"])
                    .await
                    .unwrap()
                    .status
                    .success()
            );
            assert!(
                run_git(&dir, &["config", "user.email", "test@hqcode.dev"])
                    .await
                    .unwrap()
                    .status
                    .success()
            );
            std::fs::write(dir.join("README.md"), "hello\n").unwrap();
            assert!(run_git(&dir, &["add", "README.md"]).await.unwrap().status.success());
            assert!(
                run_git(&dir, &["commit", "-m", "init"])
                    .await
                    .unwrap()
                    .status
                    .success()
            );
            Self(dir)
        }
    }

    impl Drop for TempGitRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn status_diff_revert_and_commit_flow() {
        let repo = TempGitRepo::new("flow").await;
        std::fs::write(repo.0.join("README.md"), "hello\nworld\n").unwrap();
        std::fs::write(repo.0.join("draft.txt"), "new line\n").unwrap();

        let status = git_status(&repo.0).await.unwrap();
        assert!(status.is_repo);
        assert_eq!(status.files.len(), 2);

        let diff_readme = git_diff(&repo.0, Some("README.md")).await.unwrap();
        assert!(diff_readme.diff.contains("+world"));

        let diff_draft = git_diff(&repo.0, Some("draft.txt")).await.unwrap();
        assert!(diff_draft.diff.contains("+new line"));

        // Revert untracked file and verify removal.
        git_revert_file(&repo.0, "draft.txt").await.unwrap();
        assert!(!repo.0.join("draft.txt").exists());

        // Commit modified README.md.
        let committed = git_commit(&repo.0, "update readme", None).await.unwrap();
        assert!(!committed.commit.is_empty());

        let clean = git_status(&repo.0).await.unwrap();
        assert!(clean.files.is_empty());

        // Traversal rejected.
        assert!(git_diff(&repo.0, Some("../etc/passwd")).await.is_err());
    }
}
