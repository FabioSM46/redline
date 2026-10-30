//! Helpers shared by the unit tests of the app crate.

use std::fs;
use std::path::{Path, PathBuf};

/// A test's own folder under the system temp directory, removed when the guard is dropped:
/// at the end of the test and also when the test panics, so no run leaves folders behind.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// A fresh, empty `redline-<prefix>-<name>-<pid>` folder. A leftover with the same name
    /// (a previous process with the same pid) is removed first.
    pub fn new(prefix: &str, name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("redline-{prefix}-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    #[test]
    fn the_folder_is_removed_when_the_guard_is_dropped() {
        let dir = TempDir::new("support", "drop");
        let path = dir.to_path_buf();
        fs::write(path.join("file"), "x").unwrap();
        assert!(path.is_dir());
        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn the_folder_is_removed_also_when_the_test_panics() {
        let mut path = None;
        let result = catch_unwind(AssertUnwindSafe(|| {
            let dir = TempDir::new("support", "panic");
            path = Some(dir.to_path_buf());
            panic!("a failing test");
        }));
        assert!(result.is_err());
        assert!(!path.unwrap().exists());
    }
}
