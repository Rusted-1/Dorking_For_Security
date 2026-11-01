//! Persistent run memory, kept under `<output_dir>/state/`.
//!
//! Unlike the coordination directory (short-lived claims in the temp dir),
//! this survives between runs so each run builds on the last one:
//!
//! - `queries/` — when each dork query last ran. Discovery spends its credit
//!   budget on the least-recently-run queries first, so repeated runs rotate
//!   through the whole dork list instead of re-buying the same results.
//! - `seen/` — when each site was last scanned, so discovery skips sites that
//!   were already checked recently.
//! - `reviews/` — a person's verdict on a flagged site (confirmed or false
//!   positive). Reviewed sites are never re-queued, and false positives drop
//!   out of the hit lists.
//!
//! One small file per entry (named by hash, timestamp = file mtime), the same
//! approach as `coord`, so concurrent workers never clobber each other.

use crate::coord::hash_name;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A reviewer's decision about a flagged site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Confirmed,
    FalsePositive,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Confirmed => "confirmed",
            Verdict::FalsePositive => "false_positive",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "confirmed" | "confirm" => Some(Verdict::Confirmed),
            "false_positive" | "false-positive" | "fp" => Some(Verdict::FalsePositive),
            _ => None,
        }
    }
}

pub struct State {
    dir: PathBuf,
}

impl State {
    /// State for an output directory (created lazily on first write).
    pub fn open(output_dir: &Path) -> Self {
        Self {
            dir: output_dir.join("state"),
        }
    }

    /// Write (or refresh the timestamp of) one entry.
    fn put(&self, sub: &str, key: &str, value: &str) -> Result<()> {
        let d = self.dir.join(sub);
        fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
        fs::write(d.join(hash_name(key)), format!("{key}\n{value}\n"))
            .with_context(|| format!("writing state in {}", d.display()))
    }

    /// All entries in one table: key -> (value, last written).
    fn entries(&self, sub: &str) -> HashMap<String, (String, SystemTime)> {
        let mut out = HashMap::new();
        let Ok(rd) = fs::read_dir(self.dir.join(sub)) else {
            return out;
        };
        for e in rd.flatten() {
            let Ok(txt) = fs::read_to_string(e.path()) else {
                continue;
            };
            let mut lines = txt.lines();
            let key = lines.next().unwrap_or("").trim().to_string();
            if key.is_empty() {
                continue;
            }
            let value = lines.next().unwrap_or("").trim().to_string();
            let when = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            out.insert(key, (value, when));
        }
        out
    }

    // ---- Query rotation -----------------------------------------------------

    /// Pick up to `budget` queries (0 = all): never-run first, then the ones
    /// run longest ago. Ties keep their original order.
    pub fn pick_queries(&self, queries: &[String], budget: usize) -> Vec<String> {
        let ran = self.entries("queries");
        let mut ordered: Vec<(Option<SystemTime>, usize, &String)> = queries
            .iter()
            .enumerate()
            .map(|(i, q)| (ran.get(q).map(|(_, t)| *t), i, q))
            .collect();
        // None (never run) sorts before Some(time).
        ordered.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let take = if budget == 0 { ordered.len() } else { budget };
        ordered.into_iter().take(take).map(|(_, _, q)| q.clone()).collect()
    }

    pub fn mark_query_run(&self, query: &str) {
        let _ = self.put("queries", query, "");
    }

    // ---- Scanned sites ------------------------------------------------------

    pub fn mark_seen(&self, host: &str) {
        let _ = self.put("seen", host, "");
    }

    /// Hosts scanned within the last `days` days (empty when `days` is 0).
    pub fn recently_seen(&self, days: u64) -> HashSet<String> {
        if days == 0 {
            return HashSet::new();
        }
        let window = Duration::from_secs(days * 24 * 60 * 60);
        self.entries("seen")
            .into_iter()
            .filter(|(_, (_, t))| t.elapsed().map(|d| d <= window).unwrap_or(true))
            .map(|(k, _)| k)
            .collect()
    }

    // ---- Reviews ------------------------------------------------------------

    /// Record (or with `None`, clear) a verdict for a host.
    pub fn set_review(&self, host: &str, verdict: Option<Verdict>) -> Result<()> {
        match verdict {
            Some(v) => self.put("reviews", host, v.as_str()),
            None => {
                let _ = fs::remove_file(self.dir.join("reviews").join(hash_name(host)));
                Ok(())
            }
        }
    }

    /// Every reviewed host and its verdict, sorted by host.
    pub fn reviews(&self) -> BTreeMap<String, Verdict> {
        self.entries("reviews")
            .into_iter()
            .filter_map(|(k, (v, _))| Verdict::parse(&v).map(|v| (k, v)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_state(tag: &str) -> (State, PathBuf) {
        let dir = std::env::temp_dir().join(format!("dorker-state-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        (State::open(&dir), dir)
    }

    #[test]
    fn queries_rotate_never_run_first() {
        let (s, dir) = temp_state("q");
        let qs: Vec<String> = ["a", "b", "c"].iter().map(|x| x.to_string()).collect();
        assert_eq!(s.pick_queries(&qs, 2), vec!["a", "b"]);
        s.mark_query_run("a");
        s.mark_query_run("b");
        // "c" has never run, so it goes first; then the older of a/b.
        let next = s.pick_queries(&qs, 2);
        assert_eq!(next[0], "c");
        assert_eq!(s.pick_queries(&qs, 0).len(), 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn seen_and_reviews_round_trip() {
        let (s, dir) = temp_state("r");
        s.mark_seen("scam.example");
        assert!(s.recently_seen(30).contains("scam.example"));
        assert!(s.recently_seen(0).is_empty());

        s.set_review("scam.example", Some(Verdict::Confirmed)).unwrap();
        s.set_review("legit.example", Some(Verdict::FalsePositive)).unwrap();
        let r = s.reviews();
        assert_eq!(r.get("scam.example"), Some(&Verdict::Confirmed));
        assert_eq!(r.get("legit.example"), Some(&Verdict::FalsePositive));

        s.set_review("scam.example", None).unwrap();
        assert!(!s.reviews().contains_key("scam.example"));
        let _ = fs::remove_dir_all(&dir);
    }
}
