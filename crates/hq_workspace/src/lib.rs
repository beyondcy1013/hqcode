//! Workspace filesystem boundary enforcement and lightweight Git diff/review operations.

pub mod fs_ops;
pub mod git_ops;

pub use fs_ops::{
    DirectoryEntry, DirectoryResponse, FileResponse, RenamePathResponse, SaveFileResponse,
    WorkspaceConfig, list_entries, read_utf8_file, rename_entry, resolve_directory_path,
    write_utf8_file,
};
pub use git_ops::{
    GitChangedFile, GitCommitResult, GitDiffResponse, GitStatusSummary, git_commit, git_diff,
    git_revert_file, git_status,
};
