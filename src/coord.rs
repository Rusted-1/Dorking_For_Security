//! Cross-instance coordination via a shared directory.
//!
//! Several `dorker` processes pointed at the same coordination directory stay
//! out of each other's way: before scanning a URL each one atomically "claims"
//! it (an exclusive file create), so the same page is never scanned twice. The
//! directory also carries a `STOP` sentinel that any instance can drop (via
//! `dorker stop`) and every instance checks before each page.
//!
//! This works for instances on one machine (default: a folder in the system
//! temp dir) or across machines when the directory is on a shared filesystem.

use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Stale claims older than this can be reclaimed, so a crashed instance can't
/// block a URL forever.
const CLAIM_TTL: Duration = Duration::from_secs(6 * 60 * 60);

pub struct Coordinator {
    dir: PathBuf,
}

impl Coordinator {
    /// Open (creating if needed) a coordination directory.
    pub fn open(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(dir.join("claims"))
            .with_context(|| format!("creating coordination dir {}", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    fn claim_path(&self, url: &str) -> PathBuf {
        self.dir.join("claims").join(hash_name(url))
    }

    fn stop_path(&self) -> PathBuf {
        self.dir.join("STOP")
    }

    /// Try to claim a URL. Returns true if this instance may scan it, false if
    /// another instance already has it. Fails open (true) on I/O errors so a
    /// coordination glitch never silently drops work.
    pub fn claim(&self, url: &str) -> bool {
        let path = self.claim_path(url);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                let _ = writeln!(f, "{url}");
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Reclaim if the existing claim is stale.
                if let Ok(meta) = fs::metadata(&path) {
                    if let Ok(modified) = meta.modified() {
                        if modified.elapsed().map(|d| d > CLAIM_TTL).unwrap_or(false) {
                            let _ = fs::remove_file(&path);
                            return self.claim(url);
                        }
                    }
                }
                false
            }
            Err(_) => true,
        }
    }

    /// Has a stop been requested?
    pub fn stop_requested(&self) -> bool {
        self.stop_path().exists()
    }

    /// Drop the stop sentinel so every instance halts.
    pub fn request_stop(&self) -> Result<()> {
        fs::write(self.stop_path(), b"stop")
            .with_context(|| format!("writing stop sentinel in {}", self.dir.display()))
    }

    /// Clear a leftover stop sentinel at the start of a fresh run.
    pub fn clear_stop(&self) {
        let _ = fs::remove_file(self.stop_path());
    }
}

/// Stable 64-bit FNV-1a hash as a hex filename (deterministic across processes
/// running the same binary).
fn hash_name(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}
