//! Crawl mode: start from one URL, follow same-site links, and score each page.
//!
//! Breadth-first, staying on the seed's host (ignoring a leading `www.`),
//! stopping once `max_pages` have been fetched. Fetching uses the same polite
//! settings as a normal scan (delay, timeout, robots.txt).

use crate::config::FetchConfig;
use crate::coord::Coordinator;
use crate::http::{fetch_page, looks_like_page, RobotsCache};
use crate::keywords::Matcher;
use crate::model::SiteResult;
use crate::page;
use std::collections::{HashSet, VecDeque};
use std::time::Duration;
use url::Url;

fn norm_host(u: &Url) -> Option<String> {
    u.host_str().map(|h| h.trim_start_matches("www.").to_lowercase())
}

/// A page's canonical key for de-duplication (no fragment).
fn key(u: &Url) -> String {
    let mut u = u.clone();
    u.set_fragment(None);
    u.to_string()
}

/// The results of a crawl, plus an inventory of every link seen.
pub struct CrawlOutcome {
    pub results: Vec<SiteResult>,
    /// Same-site links discovered (deduplicated, sorted).
    pub internal: Vec<String>,
    /// Off-site links discovered (deduplicated, sorted).
    pub external: Vec<String>,
    /// Set when the crawl did nothing because another instance owns the site.
    pub skipped_site: Option<String>,
}

pub async fn crawl(
    seed: &str,
    matcher: &Matcher,
    client: &reqwest::Client,
    cfg: &FetchConfig,
    max_pages: usize,
    coord: Option<&Coordinator>,
) -> CrawlOutcome {
    let seed_url = match Url::parse(seed) {
        Ok(u) => u,
        Err(e) => {
            return CrawlOutcome {
                results: vec![SiteResult {
                    url: seed.to_string(),
                    fetched: false,
                    status: None,
                    error: Some(format!("invalid start url: {e}")),
                    score: 0,
                    excluded: false,
                    exclusions: vec![],
                    hits: vec![],
                }],
                internal: vec![],
                external: vec![],
                skipped_site: None,
            }
        }
    };
    let seed_host = norm_host(&seed_url);

    // Claim the whole site (host:port). If another instance already has it,
    // do nothing. (Link-following below still scopes by host via seed_host.)
    if let Some(c) = coord {
        if let Some(site) = crate::coord::host_key(seed) {
            if !c.claim_site(&site) {
                return CrawlOutcome {
                    results: vec![],
                    internal: vec![],
                    external: vec![],
                    skipped_site: Some(site),
                };
            }
        }
    }

    // Depth 0 = the start page; depth 1 = links found on the start page.
    // We never go deeper than depth 1.
    let mut queue: VecDeque<(Url, usize)> = VecDeque::new();
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(key(&seed_url));
    queue.push_back((seed_url, 0));

    let mut results: Vec<SiteResult> = Vec::new();
    let mut internal: HashSet<String> = HashSet::new();
    let mut external: HashSet<String> = HashSet::new();
    let mut robots = RobotsCache::default();

    while let Some((url, depth)) = queue.pop_front() {
        if results.len() >= max_pages {
            break;
        }
        // Halt immediately if a stop was requested.
        if let Some(c) = coord {
            if c.stop_requested() {
                eprintln!("  stop requested — halting.");
                break;
            }
        }
        // Skip URLs another instance has already claimed.
        if let Some(c) = coord {
            if !c.claim_url(url.as_str()) {
                continue;
            }
        }
        if !results.is_empty() && cfg.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(cfg.delay_ms)).await;
        }

        if cfg.respect_robots && !robots.allows(client, &url, &cfg.user_agent).await {
            results.push(SiteResult {
                url: url.to_string(),
                fetched: false,
                status: None,
                error: Some("skipped: disallowed by robots.txt".to_string()),
                score: 0,
                excluded: false,
                exclusions: vec![],
                hits: vec![],
            });
            continue;
        }

        match fetch_page(client, url.clone()).await {
            Ok((status, body)) => {
                let page = page::parse(&body);
                drop(body);
                let (hits, exclusions) = matcher.scan(url.as_str(), &page.text);
                eprintln!("  scanned ({}/{}) {}", results.len() + 1, max_pages, url);
                results.push(SiteResult {
                    url: url.to_string(),
                    fetched: true,
                    status: Some(status),
                    error: None,
                    score: 0, // filled in by report::build_report
                    excluded: !exclusions.is_empty(),
                    exclusions,
                    hits,
                });

                // Inventory every link on this page, and (only from the start
                // page) queue same-site page links to scan next.
                for link in resolve_links(&page.links, &url) {
                    let is_internal = norm_host(&link) == seed_host;
                    if is_internal {
                        internal.insert(key(&link));
                    } else {
                        external.insert(key(&link));
                    }
                    if depth == 0 && is_internal && looks_like_page(&link) && seen.insert(key(&link)) {
                        queue.push_back((link, 1));
                    }
                }
            }
            Err((status, error)) => results.push(SiteResult {
                url: url.to_string(),
                fetched: false,
                status,
                error: Some(error),
                score: 0,
                excluded: false,
                exclusions: vec![],
                hits: vec![],
            }),
        }
    }

    let mut internal: Vec<String> = internal.into_iter().collect();
    let mut external: Vec<String> = external.into_iter().collect();
    internal.sort();
    external.sort();

    CrawlOutcome {
        results,
        internal,
        external,
        skipped_site: None,
    }
}

/// Resolve a page's hrefs against its URL, keeping absolute http(s) links.
fn resolve_links(hrefs: &[String], base: &Url) -> Vec<Url> {
    hrefs
        .iter()
        .filter_map(|href| base.join(href).ok())
        .filter(|u| u.scheme() == "http" || u.scheme() == "https")
        .map(|mut u| {
            u.set_fragment(None);
            u
        })
        .collect()
}
