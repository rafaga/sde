//! Fixture shared by the tests of the parser modules.

use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Unique temp directory with the given `.jsonl` files (name ->
/// content), removed automatically on going out of scope. Same
/// pattern as `tests/manager.rs`'s fixture.
pub(super) struct TempSdeDir {
    pub(super) path: std::path::PathBuf,
}

impl TempSdeDir {
    pub(super) fn new(test_name: &str, files: &[(&str, &str)]) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "sde_parser_test_{}_{}_{}",
            test_name,
            std::process::id(),
            id
        ));
        std::fs::create_dir_all(&path).expect("cannot create temp sde dir");
        for (name, content) in files {
            std::fs::write(path.join(name), content).expect("cannot write fixture file");
        }
        Self { path }
    }
}

impl Drop for TempSdeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
