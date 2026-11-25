//! Atomic and owner-only (`0o600`) file persistence primitives shared by HQCode
//! and compatible downstream control planes.

use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

use rand::RngCore;
use serde::{Serialize, de::DeserializeOwned};

/// Atomically replaces `path` with `contents`: writes to a unique sibling
/// temporary file, fsyncs, preserves existing destination permissions when
/// present, renames over `path`, and fsyncs the parent directory on Unix.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary = temporary_path(path, "tmp");
    write_then_replace(&temporary, path, contents)
}

/// Validates that an existing `path` is a regular file and not a symbolic link.
pub fn validate_existing_regular(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "private path must be a regular file, not a symbolic link",
        ));
    }
    Ok(())
}

/// Validates an optional file path: returns `Ok(false)` when absent, `Ok(true)`
/// for regular files, and `Err(InvalidData)` for symbolic links or directories.
pub fn validate_optional_regular(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "private path must be a regular file, not a symbolic link",
                ));
            }
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Atomically writes `contents` to `path` with owner-only (`0o600`) permissions
/// and refuses to follow or replace symbolic links.
pub fn write_private_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    validate_optional_regular(path)?;
    let temporary = temporary_path(path, "part");
    let result = write_private_new(&temporary, contents)
        .and_then(|()| replace_file(&temporary, path))
        .and_then(|()| sync_parent_directory(path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Ensures `path`'s parent directory exists (creating missing directories with
/// `0o700` on Unix) and writes `contents` via [`write_private_atomic`].
pub fn write_private_atomic_with_parent(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            builder.create(parent)?;
        }
        #[cfg(not(unix))]
        {
            fs::create_dir_all(parent)?;
        }
    }
    write_private_atomic(path, contents)
}

/// Tightens permissions on an existing regular private file to `0o600`.
pub fn tighten_if_existing_regular(path: &Path) {
    if matches!(validate_optional_regular(path), Ok(true)) {
        let _ = tighten_permissions(path);
    }
}

/// Tightens permissions on an existing regular private file and decodes JSON.
/// Missing files, symlinks, and invalid JSON return `None`.
pub fn read_private_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    tighten_if_existing_regular(path);
    if !matches!(validate_optional_regular(path), Ok(true)) {
        return None;
    }
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

/// Serializes `value` as pretty JSON and writes it atomically with `0o600`.
pub fn write_private_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write_private_atomic_with_parent(path, &bytes)
}

/// Tightens `path` to owner-only permissions (`0o600` on Unix).
#[cfg(unix)]
pub fn tighten_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    validate_existing_regular(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(windows)]
pub fn tighten_permissions(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
            SetFileSecurityW,
        },
    };

    validate_existing_regular(path)?;
    const OWNER_ONLY_SDDL: &str = "D:P(A;;FA;;;OW)(A;;FA;;;SY)(A;;FA;;;BA)";
    let sddl: Vec<u16> = OWNER_ONLY_SDDL.encode_utf16().chain(Some(0)).collect();
    let path_w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    let secured = unsafe {
        SetFileSecurityW(
            path_w.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    unsafe { LocalFree(descriptor.cast()) };
    if secured == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub fn tighten_permissions(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private files are unsupported on this platform",
    ))
}

fn temporary_path(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("atomic-file");
    let mut random = [0_u8; 8];
    rand::thread_rng().fill_bytes(&mut random);
    path.with_file_name(format!(
        ".{name}.{}-{}.{suffix}",
        std::process::id(),
        hex::encode(random)
    ))
}

fn write_then_replace(temporary: &Path, destination: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)?;
    let result = (|| {
        if let Some(metadata) = destination_metadata(destination)? {
            fs::set_permissions(temporary, metadata.permissions())?;
        }
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        replace_file(temporary, destination)?;
        sync_parent_directory(destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn destination_metadata(destination: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::metadata(destination) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn write_private_new(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(windows)]
fn write_private_new(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    tighten_permissions(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(not(any(unix, windows)))]
fn write_private_new(_path: &Path, _contents: &[u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private files are unsupported on this platform",
    ))
}

#[cfg(unix)]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{REPLACEFILE_WRITE_THROUGH, ReplaceFileW};

    if !destination.exists() {
        match fs::rename(temporary, destination) {
            Ok(()) => return Ok(()),
            Err(error) if !destination.exists() => return Err(error),
            Err(_) => {}
        }
    }

    let destination_w: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let temporary_w: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let replaced = unsafe {
        ReplaceFileW(
            destination_w.as_ptr(),
            temporary_w.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if replaced == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn replace_file(_temporary: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "file replacement is unsupported on this platform",
    ))
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "hq-fs-atomic-{label}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn temp_files(&self) -> Vec<String> {
            fs::read_dir(&self.0)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".tmp") || name.ends_with(".part"))
                .collect()
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn write_atomic_overwrites_and_cleans_up_temp_files() {
        let dir = TestDir::new("overwrite");
        let path = dir.0.join("data.txt");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert!(dir.temp_files().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_preserves_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TestDir::new("perms");
        let path = dir.0.join("data.txt");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(fs::read(&path).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn write_private_atomic_enforces_0600_and_rejects_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = TestDir::new("private");
        let secret = dir.0.join("nested").join("secret.json");
        write_private_json(&secret, &vec!["a", "b"]).unwrap();
        assert_eq!(
            fs::metadata(&secret).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let decoded: Vec<String> = read_private_json(&secret).unwrap();
        assert_eq!(decoded, vec!["a", "b"]);

        let link = dir.0.join("link.json");
        symlink(&secret, &link).unwrap();
        let err = write_private_atomic(&link, b"hijack").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(read_private_json::<Vec<String>>(&link).is_none());
    }
}
