//! Organize results into a report (JSON + readable text) and a console summary.

use crate::model::{Report, SiteResult};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

pub fn build_report(results: Vec<SiteResult>) -> Report {
    let sites_with_hits = results.iter().filter(|r| !r.hits.is_empty()).count();
    Report {
        generated_at: chrono::Local::now().to_rfc3339(),
        total_candidates: results.len(),
        sites_with_hits,
        results,
    }
}

/// Write report.json and report.txt into `output_dir`.
pub fn write_report(report: &Report, output_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("creating output dir {}", output_dir.display()))?;

    let json = serde_json::to_string_pretty(report)?;
    std::fs::write(output_dir.join("report.json"), json)
        .context("writing report.json")?;

    std::fs::write(output_dir.join("report.txt"), render_text(report))
        .context("writing report.txt")?;

    Ok(())
}

/// A human-readable report, grouped two ways: by site, then by keyword.
pub fn render_text(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("=== Dorker report ===\n");
    out.push_str(&format!("generated: {}\n", report.generated_at));
    out.push_str(&format!(
        "candidates checked: {}\nsites with hits: {}\n\n",
        report.total_candidates, report.sites_with_hits
    ));

    out.push_str("--- Sites with hits ---\n");
    for r in report.results.iter().filter(|r| !r.hits.is_empty()) {
        out.push_str(&format!("\n{}\n", r.url));
        if let Some(s) = r.status {
            out.push_str(&format!("  status: {s}\n"));
        }
        for h in &r.hits {
            let cat = h.category.as_deref().unwrap_or("-");
            out.push_str(&format!("  [{}] ({})\n", h.keyword_id, cat));
            out.push_str(&format!("      …{}…\n", h.snippet));
        }
    }

    // Index: keyword -> which sites matched it.
    let mut by_keyword: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for r in &report.results {
        for h in &r.hits {
            by_keyword.entry(&h.keyword_id).or_default().push(&r.url);
        }
    }
    if !by_keyword.is_empty() {
        out.push_str("\n--- By keyword ---\n");
        for (kw, sites) in by_keyword {
            out.push_str(&format!("\n{} ({} site(s))\n", kw, sites.len()));
            for s in sites {
                out.push_str(&format!("  - {s}\n"));
            }
        }
    }

    // Problems worth surfacing.
    let errored: Vec<&SiteResult> = report
        .results
        .iter()
        .filter(|r| r.error.is_some())
        .collect();
    if !errored.is_empty() {
        out.push_str("\n--- Skipped / errored ---\n");
        for r in errored {
            out.push_str(&format!(
                "  {}  ({})\n",
                r.url,
                r.error.as_deref().unwrap_or("unknown")
            ));
        }
    }

    out
}

/// Short summary for stdout.
pub fn console_summary(report: &Report) -> String {
    format!(
        "Checked {} candidate(s); {} had keyword hits.",
        report.total_candidates, report.sites_with_hits
    )
}
