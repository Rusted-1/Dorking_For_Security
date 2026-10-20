//! Read saved reports to summarize the highest-scoring sites found so far.

use std::path::Path;

pub struct Hit {
    pub score: u32,
    pub url: String,
}

/// The top-scoring sites across all saved reports under `out_dir`, best first.
pub fn top_hits(out_dir: &Path, limit: usize) -> Vec<Hit> {
    let mut hits = Vec::new();
    // Per-site reports written by workers.
    if let Ok(entries) = std::fs::read_dir(out_dir.join("sites")) {
        for e in entries.flatten() {
            add_report(&e.path().join("report.json"), &mut hits);
        }
    }
    // A single top-level report from a plain scan/auto, if present.
    add_report(&out_dir.join("report.json"), &mut hits);

    hits.sort_by(|a, b| b.score.cmp(&a.score));
    hits.truncate(limit);
    hits
}

/// Add the best (non-excluded) hit from one report.json file, if any.
fn add_report(path: &Path, out: &mut Vec<Hit>) {
    let Ok(txt) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
        return;
    };
    let Some(results) = v.get("results").and_then(|r| r.as_array()) else {
        return;
    };
    let mut best: Option<Hit> = None;
    for r in results {
        if r.get("excluded").and_then(|b| b.as_bool()).unwrap_or(false) {
            continue;
        }
        let score = r.get("score").and_then(|s| s.as_u64()).unwrap_or(0) as u32;
        let url = r
            .get("url")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string();
        if score == 0 || url.is_empty() {
            continue;
        }
        if best.as_ref().map(|h| score > h.score).unwrap_or(true) {
            best = Some(Hit { score, url });
        }
    }
    if let Some(h) = best {
        out.push(h);
    }
}
