//! dorker — anti-fraud dorking tool.
//!
//! Discovers candidate pages (from a URL list or an official search API),
//! checks each one for scam-boilerplate keywords, and organizes the hits
//! into a report.

mod config;
mod coord;
mod crawl;
mod dorks;
mod html;
mod http;
mod keywords;
mod model;
mod report;
mod score;
mod server;
mod sources;
mod status;
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
    /// Shared coordination directory so multiple instances don't scan the same
    /// URL twice (and so `stop` can reach them). Defaults to a folder in the
    /// system temp dir; point several machines at a shared path to coordinate.
    #[arg(long, global = true)]
    coordinate: Option<PathBuf>,
    /// Run without coordination (ignore other instances; `stop` won't reach it).
    #[arg(long, global = true)]
    solo: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

/// Resolve the coordination directory, honoring --solo.
fn coordinator(coordinate: &Option<PathBuf>, solo: bool) -> Option<coord::Coordinator> {
    if solo {
        return None;
    }
    let dir = coordinate
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("dorker-coord"));
    coord::Coordinator::open(dir).ok()
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
        /// Start URL: scans this page and every same-site link on it.
        #[arg(long)]
        url: Option<String>,
        /// Safety cap on total pages fetched (start page + its links).
        #[arg(long, default_value_t = 100)]
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
    /// Signal every running instance (sharing the coordination dir) to stop.
    Stop,
    /// List the sites currently claimed by instances (shared coordination dir).
    Sites,
    /// Clear coordination state: all site/URL claims and any stop signal.
    Reset,
    /// Auto-discover candidate sites via Google search dorks (needs a
    /// search_api source in the config). Writes out/discovered.txt.
    Discover {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        /// After discovering, scan the found sites straight through.
        #[arg(long)]
        scan: bool,
        /// Add the discovered sites to the shared work queue instead.
        #[arg(long)]
        enqueue: bool,
    },
    /// Add one or more URLs to the shared work queue.
    Enqueue {
        /// URLs to queue (each needs https://).
        urls: Vec<String>,
        /// Also read URLs from a file, one per line.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Show the shared work queue (pending + in-progress).
    Queue,
    /// One-glance status: queue, positives, claimed sites, and top hits.
    Status {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Worker: pull sites from the queue and scan them until it's empty.
    Work {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        /// Safety cap on pages per site.
        #[arg(long, default_value_t = 100)]
        max: usize,
        /// Keep running and poll for new queue items instead of exiting.
        #[arg(long)]
        watch: bool,
        /// Stop once this many positive sites have been found (shared across
        /// all instances). Omit for no limit.
        #[arg(long)]
        max_positives: Option<usize>,
        /// A site counts as a positive when its top score is at least this.
        #[arg(long, default_value_t = 50)]
        positive_threshold: u32,
    },
    /// Guided setup: answer a few questions and write config.toml for you.
    Setup {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// One command for the whole pipeline: discover sites, then scan them all.
    Auto {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        /// Safety cap on pages per site.
        #[arg(long, default_value_t = 100)]
        max: usize,
        /// Stop once this many positive sites have been found. Omit for no limit.
        #[arg(long)]
        max_positives: Option<usize>,
        /// A site counts as a positive when its top score is at least this.
        #[arg(long, default_value_t = 50)]
        positive_threshold: u32,
    },
    /// Start the local web control panel (buttons + live status) in a browser.
    Serve {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let color = use_color(cli.color);
    match cli.cmd {
        Cmd::Scan { url, max, config } => {
            let coord = coordinator(&cli.coordinate, cli.solo);
            run_scan(url, max, &config, color, coord.as_ref()).await
        }
        Cmd::Check {
            url,
            file,
            keywords,
        } => run_check(url, file, &keywords, color).await,
        Cmd::Stop => {
            let c = coordinator(&cli.coordinate, false)
                .ok_or_else(|| anyhow::anyhow!("could not open coordination directory"))?;
            c.request_stop()?;
            println!(
                "Stop signal sent. Running instances using {} will halt before their next page.",
                c.path().display()
            );
            Ok(())
        }
        Cmd::Sites => {
            let c = coordinator(&cli.coordinate, false)
                .ok_or_else(|| anyhow::anyhow!("could not open coordination directory"))?;
            let sites = c.list_sites();
            if sites.is_empty() {
                println!("No sites currently claimed ({}).", c.path().display());
            } else {
                println!("Claimed sites ({}):", sites.len());
                for s in sites {
                    println!("  {s}");
                }
            }
            Ok(())
        }
        Cmd::Reset => {
            let c = coordinator(&cli.coordinate, false)
                .ok_or_else(|| anyhow::anyhow!("could not open coordination directory"))?;
            c.reset()?;
            println!("Coordination state cleared in {}.", c.path().display());
            Ok(())
        }
        Cmd::Discover { config, scan, enqueue } => {
            let coord = coordinator(&cli.coordinate, cli.solo);
            run_discover(&config, scan, enqueue, color, coord.as_ref()).await
        }
        Cmd::Enqueue { urls, file } => {
            let c = coordinator(&cli.coordinate, false)
                .ok_or_else(|| anyhow::anyhow!("could not open coordination directory"))?;
            let mut all = urls;
            if let Some(f) = file {
                let raw = std::fs::read_to_string(&f)
                    .with_context(|| format!("reading {}", f.display()))?;
                all.extend(
                    raw.lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty() && !l.starts_with('#'))
                        .map(String::from),
                );
            }
            if all.is_empty() {
                anyhow::bail!("nothing to enqueue — pass URLs or --file");
            }
            let (mut added, mut dup) = (0usize, 0usize);
            for u in &all {
                if c.enqueue(u) {
                    added += 1;
                } else {
                    dup += 1;
                }
            }
            println!("Queued {added} URL(s) ({dup} already present) in {}.", c.path().display());
            Ok(())
        }
        Cmd::Queue => {
            let c = coordinator(&cli.coordinate, false)
                .ok_or_else(|| anyhow::anyhow!("could not open coordination directory"))?;
            let (pending, processing) = c.queue_counts();
            println!("Queue: {pending} pending, {processing} in progress ({}).", c.path().display());
            for u in c.queue_list() {
                println!("  {u}");
            }
            Ok(())
        }
        Cmd::Work { config, max, watch, max_positives, positive_threshold } => {
            let coord = coordinator(&cli.coordinate, cli.solo)
                .ok_or_else(|| anyhow::anyhow!("worker needs a coordination directory (don't use --solo)"))?;
            run_work(&config, max, watch, max_positives, positive_threshold, &coord).await
        }
        Cmd::Status { config } => {
            let c = coordinator(&cli.coordinate, false)
                .ok_or_else(|| anyhow::anyhow!("could not open coordination directory"))?;
            let output_dir = config::load_config(&config)
                .ok()
                .map(|cfg| cfg.output_dir)
                .unwrap_or_else(|| "out".to_string());
            run_status(&c, &output_dir, color);
            Ok(())
        }
        Cmd::Setup { config } => run_setup(&config),
        Cmd::Auto { config, max, max_positives, positive_threshold } => {
            let coord = coordinator(&cli.coordinate, cli.solo)
                .ok_or_else(|| anyhow::anyhow!("auto needs a coordination directory (don't use --solo)"))?;
            run_auto(&config, max, max_positives, positive_threshold, color, &coord).await
        }
        Cmd::Serve { config, port } => {
            let coord = coordinator(&cli.coordinate, false)
                .ok_or_else(|| anyhow::anyhow!("the panel needs a coordination directory (don't use --solo)"))?;
            let output_dir = config::load_config(&config)
                .ok()
                .map(|cfg| cfg.output_dir)
                .unwrap_or_else(|| "out".to_string());
            server::run_serve(port, &config, &output_dir, &coord)
        }
    }
}

/// Print a one-glance status: queue, positives, claimed sites, top hits.
fn run_status(coord: &coord::Coordinator, output_dir: &str, color: bool) {
    let (pending, processing) = coord.queue_counts();
    let positives = coord.positive_list();
    let sites = coord.list_sites();

    println!("Dorker status   (coordination: {})", coord.path().display());
    if coord.stop_requested() {
        println!("  ** a STOP signal is currently set — runs will halt. Use `reset` to clear. **");
    }
    println!();
    println!("  Queue:          {pending} pending, {processing} in progress");
    println!("  Positives:      {} flagged", positives.len());
    println!("  Sites claimed:  {}", sites.len());

    let hits = status::top_hits(Path::new(output_dir), 10);
    println!("\n  Top hits (from reports in {output_dir}/):");
    if hits.is_empty() {
        println!("    (none yet)");
    } else {
        for h in &hits {
            println!("    {}  {}", score::tag(h.score, color), h.url);
        }
    }

    if !positives.is_empty() {
        println!("\n  Flagged sites:");
        for p in &positives {
            println!("    {p}");
        }
    }
}

/// Read one line from stdin, returning `default` if the user just hits Enter.
fn prompt(label: &str, default: &str) -> String {
    use std::io::Write;
    if default.is_empty() {
        print!("{label}: ");
    } else {
        print!("{label} [{default}]: ");
    }
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    if std::io::stdin().read_line(&mut s).is_err() {
        return default.to_string();
    }
    let s = s.trim();
    if s.is_empty() {
        default.to_string()
    } else {
        s.to_string()
    }
}

/// Guided setup: ask a few questions and write config.toml.
fn run_setup(config_path: &Path) -> Result<()> {
    println!("Dorker setup — this writes {}.\n", config_path.display());

    if config_path.exists() {
        let ans = prompt(
            &format!("{} already exists. Overwrite it? (y/N)", config_path.display()),
            "N",
        );
        if !ans.eq_ignore_ascii_case("y") {
            println!("Left your config untouched.");
            return Ok(());
        }
    }

    println!("\nAuto-discovery finds scam sites for you using Google search.");
    println!("It needs a free Google key + search-engine id. Leave the key blank");
    println!("to skip discovery for now (you can re-run setup later).\n");

    let api_key = prompt("Google API key (blank to skip)", "");
    let search_api = !api_key.is_empty();
    let cx = if search_api {
        prompt("Google search-engine id (cx)", "")
    } else {
        String::new()
    };

    println!("\nPoliteness: how long to wait between page requests.");
    let delay = prompt("Delay in milliseconds", "1500");
    let output_dir = prompt("Folder for reports", "out");

    let source = if search_api {
        format!(
            "[source]\nkind = \"search_api\"\nprovider = \"google_cse\"\n\
             api_key = \"{api_key}\"\ncx = \"{cx}\"\nmax_results = 50\n\
             # queries empty = auto-build dorks from keywords.json\nqueries = []\n"
        )
    } else {
        "[source]\nkind = \"url_list\"\npath = \"candidates.txt\"\n".to_string()
    };

    let contents = format!(
        "# Written by `dorker setup`.\n\
         keywords_file = \"keywords.json\"\n\
         output_dir = \"{output_dir}\"\n\n\
         [fetch]\n\
         delay_ms = {delay}\n\
         timeout_secs = 20\n\
         user_agent = \"Dorker/0.1 (+anti-fraud research)\"\n\
         respect_robots = true\n\n\
         {source}"
    );

    std::fs::write(config_path, contents)
        .with_context(|| format!("writing {}", config_path.display()))?;

    println!("\nSaved {}.", config_path.display());
    if search_api {
        println!("You're set for auto mode. Run:  dorker auto");
    } else {
        println!("No discovery key set. You can:");
        println!("  • scan one site:   dorker scan --url https://suspect-site.com/");
        println!("  • or add URLs to candidates.txt and run:  dorker scan --config {}", config_path.display());
        println!("Re-run `dorker setup` any time to add a Google key for auto-discovery.");
    }
    Ok(())
}

/// One-command pipeline: discover sites and scan them all.
async fn run_auto(
    config_path: &Path,
    max: usize,
    max_positives: Option<usize>,
    positive_threshold: u32,
    color: bool,
    coord: &coord::Coordinator,
) -> Result<()> {
    // Friendly pre-check so a missing key gives guidance, not a raw error.
    match config::load_config(config_path) {
        Ok(cfg) if matches!(cfg.source, config::SourceConfig::SearchApi { .. }) => {}
        Ok(_) => anyhow::bail!(
            "auto mode discovers sites via search, but {} isn't set up for it.\n\
             Run `dorker setup` and add your Google key, or scan one site with \
             `dorker scan --url https://…`.",
            config_path.display()
        ),
        Err(_) => anyhow::bail!(
            "no config found at {}. Run `dorker setup` first.",
            config_path.display()
        ),
    }

    println!("Step 1 of 2 — discovering candidate sites…");
    run_discover(config_path, false, true, color, Some(coord)).await?;
    println!("\nStep 2 of 2 — scanning the queued sites…");
    run_work(config_path, max, false, max_positives, positive_threshold, coord).await?;
    println!("\nAll done. Open the reports in your browser — see out/sites/<host>/report.html");
    Ok(())
}

async fn run_scan(
    url: Option<String>,
    max: usize,
    config_path: &Path,
    color: bool,
    coord: Option<&coord::Coordinator>,
) -> Result<()> {
    // Start fresh: clear any leftover stop sentinel from a previous run.
    if let Some(c) = coord {
        c.clear_stop();
    }
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
    let matcher = keywords::Matcher::new(kws.clone())?;
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

    let out_dir = PathBuf::from(&output_dir);

    let results;
    let mut link_writeup = None;
    if let Some(seed) = &url {
        println!("Scanning {seed} and the links on it (safety cap {max} pages)…");
        let outcome = crawl::crawl(seed, &matcher, &client, &fetch, max, coord).await;
        if let Some(host) = &outcome.skipped_site {
            println!("Site '{host}' is already being handled by another instance — skipping.");
            return Ok(());
        }
        link_writeup = Some((seed.clone(), outcome.internal, outcome.external));
        results = outcome.results;
    } else {
        let cfg = cfg_opt
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no config file found and no --url given"))?;
        println!("Gathering candidates…");
        let candidates = sources::gather_candidates(&cfg.source, &client, &kws).await?;
        println!("Found {} candidate URL(s).", candidates.len());
        results = verify::verify_sites(&candidates, &matcher, &client, &fetch, coord).await;
    }

    let halted = coord.map(|c| c.stop_requested()).unwrap_or(false);

    let report = report::build_report(results);
    report::write_report(&report, &out_dir)?;

    if let Some((seed, internal, external)) = &link_writeup {
        report::write_links(seed, internal, external, &out_dir)?;
    }

    println!("{}", report::console_summary(&report));
    println!("{}", report::console_ranked(&report, color));
    println!(
        "Reports written to {d}/report.html, {d}/report.json and {d}/report.txt",
        d = output_dir
    );
    if let Some((_, internal, external)) = &link_writeup {
        println!(
            "Link write-up: {output_dir}/links.md ({} internal, {} external)",
            internal.len(),
            external.len()
        );
    }
    if halted {
        println!("Halted early by a stop signal — the report covers pages scanned so far.");
    }
    Ok(())
}

async fn run_discover(
    config_path: &Path,
    scan: bool,
    enqueue: bool,
    color: bool,
    coord: Option<&coord::Coordinator>,
) -> Result<()> {
    let cfg = config::load_config(config_path)?;
    if !matches!(cfg.source, config::SourceConfig::SearchApi { .. }) {
        anyhow::bail!(
            "discover needs a [source] with kind = \"search_api\" in {}",
            config_path.display()
        );
    }

    let kws = keywords::load_keywords(Path::new(&cfg.keywords_file))?;
    let matcher = keywords::Matcher::new(kws.clone())?;
    if matcher.is_empty() {
        anyhow::bail!("no keywords loaded from {}", cfg.keywords_file);
    }
    println!("Loaded {} keyword(s).", matcher.len());

    let client = http::build_client(&cfg.fetch)?;
    let out_dir = PathBuf::from(&cfg.output_dir);
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("creating output dir {}", out_dir.display()))?;

    println!("Discovering candidate sites…");
    let candidates = sources::gather_candidates(&cfg.source, &client, &kws).await?;

    let list_path = out_dir.join("discovered.txt");
    let body: String = candidates
        .iter()
        .map(|u| format!("{u}\n"))
        .collect::<String>();
    std::fs::write(&list_path, format!("# Discovered by dorker\n{body}"))
        .with_context(|| format!("writing {}", list_path.display()))?;
    println!(
        "Discovered {} candidate URL(s) → {}",
        candidates.len(),
        list_path.display()
    );

    if enqueue {
        let c = coord
            .ok_or_else(|| anyhow::anyhow!("--enqueue needs a coordination directory (don't use --solo)"))?;
        let (mut added, mut dup) = (0usize, 0usize);
        for u in &candidates {
            if c.enqueue(u) {
                added += 1;
            } else {
                dup += 1;
            }
        }
        println!("Added {added} to the work queue ({dup} already present). Run `dorker work` to process them.");
        return Ok(());
    }

    if !scan {
        println!("Run `dorker scan` on these (or re-run with --scan) to score them.");
        return Ok(());
    }
    if candidates.is_empty() {
        println!("Nothing to scan.");
        return Ok(());
    }

    if let Some(c) = coord {
        c.clear_stop();
    }
    println!("Scanning {} discovered candidate(s)…", candidates.len());
    let results = verify::verify_sites(&candidates, &matcher, &client, &cfg.fetch, coord).await;
    let halted = coord.map(|c| c.stop_requested()).unwrap_or(false);

    let report = report::build_report(results);
    report::write_report(&report, &out_dir)?;
    println!("{}", report::console_summary(&report));
    println!("{}", report::console_ranked(&report, color));
    println!(
        "Reports written to {d}/report.html, {d}/report.json and {d}/report.txt",
        d = cfg.output_dir
    );
    if halted {
        println!("Halted early by a stop signal — the report covers pages scanned so far.");
    }
    Ok(())
}

/// Filesystem-safe folder name for a host (for per-site report dirs).
fn safe_host(url: &str) -> String {
    let host = coord::host_key(url).unwrap_or_else(|| "site".to_string());
    host.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect()
}

async fn run_work(
    config_path: &Path,
    max: usize,
    watch: bool,
    max_positives: Option<usize>,
    positive_threshold: u32,
    coord: &coord::Coordinator,
) -> Result<()> {
    coord.clear_stop();

    // Config is optional; fall back to built-in defaults like crawl mode.
    let cfg_opt = config::load_config(config_path).ok();
    let fetch = cfg_opt.as_ref().map(|c| c.fetch.clone()).unwrap_or_default();
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
    let client = http::build_client(&fetch)?;

    let (pending, processing) = coord.queue_counts();
    println!(
        "Worker started. Queue: {pending} pending, {processing} in progress. Reports → {output_dir}/sites/<host>/"
    );
    if let Some(cap) = max_positives {
        println!("Will stop after {cap} positive(s) (score ≥ {positive_threshold}).");
    }

    let mut done = 0usize;
    loop {
        if coord.stop_requested() {
            println!("Stop requested — worker halting.");
            break;
        }
        // Honor the shared positives cap before taking more work.
        if let Some(cap) = max_positives {
            if coord.positive_count() >= cap {
                println!("Positive limit reached ({cap}) — worker halting.");
                break;
            }
        }
        let url = match coord.take_next() {
            Some(u) => u,
            None => {
                if watch {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                } else {
                    break;
                }
            }
        };

        println!("→ taking {url}");
        let outcome = crawl::crawl(&url, &matcher, &client, &fetch, max, Some(coord)).await;
        if let Some(host) = &outcome.skipped_site {
            println!("  (site '{host}' already handled by another instance; skipping)");
            coord.complete(&url);
            continue;
        }

        let site_dir = PathBuf::from(&output_dir).join("sites").join(safe_host(&url));
        let report = report::build_report(outcome.results);
        report::write_report(&report, &site_dir)?;
        report::write_links(&url, &outcome.internal, &outcome.external, &site_dir)?;
        println!(
            "  {} → {}",
            report::console_summary(&report),
            site_dir.display()
        );
        coord.complete(&url);
        done += 1;

        // Count a positive and enforce the shared cap.
        let top = report
            .results
            .iter()
            .filter(|r| !r.excluded)
            .map(|r| r.score)
            .max()
            .unwrap_or(0);
        if top >= positive_threshold {
            if let Some(host) = coord::host_key(&url) {
                let count = coord.record_positive(&host);
                println!("  positive #{count} ({top}/100)");
                if let Some(cap) = max_positives {
                    if count >= cap {
                        println!("Positive limit reached ({cap}) — signaling all instances to stop.");
                        let _ = coord.request_stop();
                        break;
                    }
                }
            }
        }
    }

    println!("Worker finished — processed {done} site(s).");
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
