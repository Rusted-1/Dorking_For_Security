//! Cross-instance coordination via a shared directory.
//!
//! Several `dorker` processes pointed at the same coordination directory stay
//! out of each other's way. Each instance **claims a whole site** (its host)
//! before scanning it; any other instance that sees the site already claimed
//! stays off it entirely. Per-URL claims add a second guard. The directory
//! also carries a `STOP` sentinel that any instance can drop (`dorker stop`)
//! and every instance checks before each page.
//!
//! Works for instances on one machine (default: a folder in the system temp
//! dir) or across machines when the directory is on a shared filesystem.

use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

/// Stale claims older than this can be reclaimed, so a crashed instance can't
/// block a site forever.
const CLAIM_TTL: Duration = Duration::from_secs(6 * 60 * 60);

pub struct Coordinator {
    dir: PathBuf,
}

impl Coordinator {
    /// Open (creating if needed) a coordination directory.
    pub fn open(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(dir.join("claims"))
            .with_context(|| format!("creating coordination dir {}", dir.display()))?;
        fs::create_dir_all(dir.join("sites"))
            .with_context(|| format!("creating coordination dir {}", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Try to claim one key under `subdir`, writing `contents` into the file.
    /// Returns true on success, false if already held (and not stale). Fails
    /// open (true) on I/O errors so a glitch never silently drops work.
    fn claim_in(&self, subdir: &str, key: &str, contents: &str) -> bool {
        let path = self.dir.join(subdir).join(hash_name(key));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                let _ = writeln!(f, "{contents}");
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Ok(meta) = fs::metadata(&path) {
                    if let Ok(modified) = meta.modified() {
                        if modified.elapsed().map(|d| d > CLAIM_TTL).unwrap_or(false) {
                            let _ = fs::remove_file(&path);
                            return self.claim_in(subdir, key, contents);
                        }
                    }
                }
                false
            }
            Err(_) => true,
        }
    }

    /// Claim a whole site (host). Returns false if another instance has it.
    pub fn claim_site(&self, host: &str) -> bool {
        self.claim_in("sites", host, host)
    }

    /// Claim a single URL (page-level guard).
    pub fn claim_url(&self, url: &str) -> bool {
        self.claim_in("claims", url, url)
    }

    /// List the sites currently claimed (their host names).
    pub fn list_sites(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(self.dir.join("sites")) {
            for e in entries.flatten() {
                if let Ok(contents) = fs::read_to_string(e.path()) {
                    if let Some(line) = contents.lines().next() {
                        out.push(line.trim().to_string());
                    }
                }
            }
        }
        out.sort();
        out
    }

    fn stop_path(&self) -> PathBuf {
        self.dir.join("STOP")
    }

    pub fn stop_requested(&self) -> bool {
        self.stop_path().exists()
    }

    pub fn request_stop(&self) -> Result<()> {
        fs::write(self.stop_path(), b"stop")
            .with_context(|| format!("writing stop sentinel in {}", self.dir.display()))
    }

    pub fn clear_stop(&self) {
        let _ = fs::remove_file(self.stop_path());
    }

    /// Clear all coordination state: site claims, URL claims, and any stop.
    pub fn reset(&self) -> Result<()> {
        let _ = fs::remove_dir_all(self.dir.join("sites"));
        let _ = fs::remove_dir_all(self.dir.join("claims"));
        self.clear_stop();
        fs::create_dir_all(self.dir.join("claims"))?;
        fs::create_dir_all(self.dir.join("sites"))?;
        Ok(())
    }
}

/// Normalized host of a URL (lowercased, leading `www.` stripped).
pub fn host_key(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_lowercase()))
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
