//! Filesystem permission lockdown (D37): every file/dir agentrec writes under
//! `.agentrec/` must end up mode 0600 (files) / 0700 (dirs), independent of
//! the process umask. `OpenOptions::mode(...)` is masked by umask at create
//! time, so a post-create `set_permissions` call is the only umask-
//! independent way to guarantee this — call these helpers once the path
//! already exists. Never fails the caller: a permission-lockdown failure
//! (e.g. a non-POSIX filesystem) is a warning, not an error (D37).

use std::path::Path;

#[cfg(unix)]
pub fn lock_file(path: &Path) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        eprintln!(
            "agentrec: could not lock down permissions on {}: {e}",
            path.display()
        );
    }
}

#[cfg(unix)]
pub fn lock_dir(path: &Path) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o700)) {
        eprintln!(
            "agentrec: could not lock down permissions on {}: {e}",
            path.display()
        );
    }
}

#[cfg(not(unix))]
pub fn lock_file(_path: &Path) {}

#[cfg(not(unix))]
pub fn lock_dir(_path: &Path) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn lock_file_sets_0600_regardless_of_create_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        fs::write(&path, b"x").unwrap();
        // default umask leaves this at 644 (or similar) — assert the fix matters.
        lock_file(&path);
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn lock_dir_sets_0700() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("d");
        fs::create_dir(&path).unwrap();
        lock_dir(&path);
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
