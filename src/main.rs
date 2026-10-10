//! dorker — anti-fraud dorking tool.
//!
//! Discovers candidate pages (from a URL list or an official search API),
//! checks each one for scam-boilerplate keywords, and organizes the hits
//! into a report.

mod config;
mod http;
mod keywords;
mod model;
mod report;
mod score;
mod sources;
mod verify;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "dorker", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a full scan from a config file.
    Scan {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Check a single page (a URL or a local HTML file) against the keywords.
    /// Handy for testing your keyword list offline.
    Check {
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long, default_value = "keywords.json")]
        keywords: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Scan { config } => run_scan(&config).await,
        Cmd::Check {
            url,
            file,
            keywords,
        } => run_check(url, file, &keywords).await,
    }
}

async fn run_scan(config_path: &Path) -> Result<()> {
    let cfg = config::load_config(config_path)?;
    let kws = keywords::load_keywords(Path::new(&cfg.keywords_file))?;
    let matcher = keywords::Matcher::new(kws)?;
    if matcher.is_empty() {
        anyhow::bail!("no keywords loaded from {}", cfg.keywords_file);
    }
    let (content_kw, url_kw, exclude_kw) = matcher.counts();
    println!(
        "Loaded {} keyword(s): {} content, {} url, {} exclusion.",
        matcher.len(),
        content_kw,
        url_kw,
        exclude_kw
    );

    let client = http::build_client(&cfg.fetch)?;

    println!("Gathering candidates…");
    let candidates = sources::gather_candidates(&cfg.source, &client).await?;
    println!("Found {} candidate URL(s).", candidates.len());

    let results = verify::verify_sites(&candidates, &matcher, &client, &cfg.fetch).await;
    let report = report::build_report(results);

    let out_dir = PathBuf::from(&cfg.output_dir);
    report::write_report(&report, &out_dir)?;

    println!("{}", report::console_summary(&report));
    println!("Report written to {}/report.json and {}/report.txt", cfg.output_dir, cfg.output_dir);
    Ok(())
}

async fn run_check(url: Option<String>, file: Option<PathBuf>, keywords_path: &Path) -> Result<()> {
    let kws = keywords::load_keywords(keywords_path)?;
    let matcher = keywords::Matcher::new(kws)?;

    let (label, url_for_match, html) = match (url, file) {
        (Some(u), None) => {
            let cfg = config::FetchConfig::default();
            let client = http::build_client(&cfg)?;
            let body = client
                .get(&u)
                .send()
                .await
                .context("fetching url")?
                .text()
                .await
                .context("reading body")?;
            (u.clone(), u, body)
        }
        (None, Some(f)) => {
            let body = std::fs::read_to_string(&f)
                .with_context(|| format!("reading {}", f.display()))?;
            (f.display().to_string(), String::new(), body)
        }
        _ => anyhow::bail!("pass exactly one of --url or --file"),
    };

    let text = verify::extract_text(&html);
    let hits = matcher.find(&url_for_match, &text);
    let exclusions = matcher.exclusions(&url_for_match, &text);

    println!("Checked: {label}");
    if !exclusions.is_empty() {
        println!(
            "EXCLUDED (treated as legitimate) — matched: {}",
            exclusions.join(", ")
        );
        println!("Score: 0/100 (excluded)");
        return Ok(());
    }
    if hits.is_empty() {
        println!("No keyword hits. Score: 0/100 (none)");
    } else {
        let total: u32 = hits.iter().map(|h| h.weight).sum();
        let s = score::certainty(total);
        println!("Score: {}/100 ({})", s, score::band(s));
        println!("{} hit(s):", hits.len());
        for h in hits {
            let cat = h.category.as_deref().unwrap_or("-");
            println!(
                "  [{}] ({}, in:{}, +{})  …{}…",
                h.keyword_id, cat, h.matched_in, h.weight, h.snippet
            );
        }
    }
    Ok(())
}
