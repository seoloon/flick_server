//! Settings edited at runtime: the stored file layered over the environment.

pub mod store;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// A fresh empty directory under the system temp dir (for tests; not removed afterwards).
pub fn scratch_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "flick-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("cannot create a scratch directory");
    dir
}
