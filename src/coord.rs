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
        for sub in ["claims", "sites", "queue", "queue_proc", "positives"] {
            fs::create_dir_all(dir.join(sub))
                .with_context(|| format!("creating coordination dir {}", dir.display()))?;
        }
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

    /// Clear all coordination state: site claims, URL claims, queue, and stop.
    pub fn reset(&self) -> Result<()> {
        for sub in ["sites", "claims", "queue", "queue_proc", "positives"] {
            let _ = fs::remove_dir_all(self.dir.join(sub));
            fs::create_dir_all(self.dir.join(sub))?;
        }
        self.clear_stop();
        Ok(())
    }

    // ---- Positives (shared cap + catalog) -----------------------------------

    /// Record a site flagged as a positive. Deduplicated by host. Returns the
    /// current total count of distinct positive sites.
    pub fn record_positive(&self, host: &str) -> usize {
        let _ = self.claim_in("positives", host, host);
        self.positive_count()
    }

    /// How many distinct positive sites have been recorded.
    pub fn positive_count(&self) -> usize {
        count_dir(&self.dir.join("positives"))
    }

    /// The hosts recorded as positives, sorted.
    #[allow(dead_code)] // used by the forthcoming status view
    pub fn positive_list(&self) -> Vec<String> {
        read_url_files(&self.dir.join("positives"))
    }

    // ---- Work queue ---------------------------------------------------------

    /// Add a URL to the shared work queue. Returns false if it's already queued.
    pub fn enqueue(&self, url: &str) -> bool {
        self.claim_in("queue", url, url)
    }

    /// Pending queue items (URLs), sorted.
    pub fn queue_list(&self) -> Vec<String> {
        read_url_files(&self.dir.join("queue"))
    }

    /// (pending, in-progress) counts.
    pub fn queue_counts(&self) -> (usize, usize) {
        let pending = count_dir(&self.dir.join("queue"));
        let processing = count_dir(&self.dir.join("queue_proc"));
        (pending, processing)
    }

    /// Atomically take the next queue item, moving it to in-progress. Returns
    /// the URL, or None when the queue is empty. Safe for many workers: the
    /// rename succeeds for exactly one of them.
    pub fn take_next(&self) -> Option<String> {
        self.reclaim_stale();
        let qdir = self.dir.join("queue");
        let pdir = self.dir.join("queue_proc");
        let mut names: Vec<_> = fs::read_dir(&qdir)
            .ok()?
            .flatten()
            .map(|e| e.file_name())
            .collect();
        names.sort();
        for name in names {
            let from = qdir.join(&name);
            let to = pdir.join(&name);
            if fs::rename(&from, &to).is_ok() {
                if let Ok(c) = fs::read_to_string(&to) {
                    return Some(c.lines().next().unwrap_or("").trim().to_string());
                }
            }
        }
        None
    }

    /// Mark a taken item finished (remove its in-progress marker).
    pub fn complete(&self, url: &str) {
        let _ = fs::remove_file(self.dir.join("queue_proc").join(hash_name(url)));
    }

    /// Move in-progress items older than the TTL back to the queue (a worker
    /// that crashed mid-item shouldn't strand the site).
    fn reclaim_stale(&self) {
        let pdir = self.dir.join("queue_proc");
        let qdir = self.dir.join("queue");
        if let Ok(entries) = fs::read_dir(&pdir) {
            for e in entries.flatten() {
                if let Ok(meta) = e.metadata() {
                    if let Ok(modified) = meta.modified() {
                        if modified.elapsed().map(|d| d > CLAIM_TTL).unwrap_or(false) {
                            let _ = fs::rename(e.path(), qdir.join(e.file_name()));
                        }
                    }
                }
            }
        }
    }
}

fn read_url_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            if let Ok(c) = fs::read_to_string(e.path()) {
                if let Some(line) = c.lines().next() {
                    out.push(line.trim().to_string());
                }
            }
        }
    }
    out.sort();
    out
}

fn count_dir(dir: &Path) -> usize {
    fs::read_dir(dir).map(|e| e.flatten().count()).unwrap_or(0)
}

/// Filesystem-safe folder name for a URL's site (used for per-site report
/// directories, e.g. `out/sites/<safe_host>/`).
pub fn safe_host(url: &str) -> String {
    let host = host_key(url).unwrap_or_else(|| "site".to_string());
    host.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect()
}

/// Normalized site identity of a URL: lowercased host (leading `www.`
/// stripped), with the port appended when the URL specifies one. Two services
/// on the same host but different ports are distinct sites.
pub fn host_key(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    let host = u.host_str()?.trim_start_matches("www.").to_lowercase();
    match u.port() {
        Some(p) => Some(format!("{host}:{p}")),
        None => Some(host),
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
