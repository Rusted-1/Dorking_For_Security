//! dorker — anti-fraud dorking tool.
//!
//! Discovers candidate pages (from a URL list or an official search API),
//! checks each one for scam-boilerplate keywords, and organizes the hits
//! into a report.

mod config;
mod crawl;
mod html;
mod http;
mod keywords;
mod model;
mod report;
mod score;
mod sources;
mod verify;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "dorker", version, about)]
struct Cli {
    /// When to colorize terminal output.
    #[arg(long, value_enum, default_value_t = ColorWhen::Auto, global = true)]
    color: ColorWhen,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Copy, Clone, ValueEnum)]
enum ColorWhen {
    Auto,
    Always,
    Never,
}

fn use_color(when: ColorWhen) -> bool {
    use std::io::IsTerminal;
    match when {
        ColorWhen::Always => true,
        ColorWhen::Never => false,
        ColorWhen::Auto => {
            std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a scan. Give --url to crawl one site, or use a config's source.
    Scan {
        /// Start URL to crawl (follows same-site links automatically).
        #[arg(long)]
        url: Option<String>,
        /// Maximum pages to fetch when crawling with --url.
        #[arg(long, default_value_t = 20)]
        max: usize,
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
    let color = use_color(cli.color);
    match cli.cmd {
        Cmd::Scan { url, max, config } => run_scan(url, max, &config, color).await,
        Cmd::Check {
            url,
            file,
            keywords,
        } => run_check(url, file, &keywords, color).await,
    }
}

async fn run_scan(
    url: Option<String>,
    max: usize,
    config_path: &Path,
    color: bool,
) -> Result<()> {
    // A config is required for a source-based scan, but optional when --url is
    // given (crawl mode can run with built-in defaults).
    let cfg_opt = match config::load_config(config_path) {
        Ok(c) => Some(c),
        Err(e) => {
            if url.is_some() {
                None
            } else {
                return Err(e);
            }
        }
    };

    let fetch = cfg_opt
        .as_ref()
        .map(|c| c.fetch.clone())
        .unwrap_or_default();
    let keywords_file = cfg_opt
        .as_ref()
        .map(|c| c.keywords_file.clone())
        .unwrap_or_else(|| "keywords.json".to_string());
    let output_dir = cfg_opt
        .as_ref()
        .map(|c| c.output_dir.clone())
        .unwrap_or_else(|| "out".to_string());

    let kws = keywords::load_keywords(Path::new(&keywords_file))?;
    let matcher = keywords::Matcher::new(kws)?;
    if matcher.is_empty() {
        anyhow::bail!("no keywords loaded from {}", keywords_file);
    }
    let (content_kw, url_kw, exclude_kw) = matcher.counts();
    println!(
        "Loaded {} keyword(s): {} content, {} url, {} exclusion.",
        matcher.len(),
        content_kw,
        url_kw,
        exclude_kw
    );

    let client = http::build_client(&fetch)?;

    let results = if let Some(seed) = &url {
        println!("Crawling {seed} (up to {max} page(s))…");
        crawl::crawl(seed, &matcher, &client, &fetch, max).await
    } else {
        let cfg = cfg_opt
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no config file found and no --url given"))?;
        println!("Gathering candidates…");
        let candidates = sources::gather_candidates(&cfg.source, &client).await?;
        println!("Found {} candidate URL(s).", candidates.len());
        verify::verify_sites(&candidates, &matcher, &client, &fetch).await
    };

    let report = report::build_report(results);

    let out_dir = PathBuf::from(&output_dir);
    report::write_report(&report, &out_dir)?;

    println!("{}", report::console_summary(&report));
    println!("{}", report::console_ranked(&report, color));
    println!(
        "Reports written to {d}/report.html, {d}/report.json and {d}/report.txt",
        d = output_dir
    );
    Ok(())
}

async fn run_check(
    url: Option<String>,
    file: Option<PathBuf>,
    keywords_path: &Path,
    color: bool,
) -> Result<()> {
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
        println!("Score: {}", score::tag(s, color));
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
