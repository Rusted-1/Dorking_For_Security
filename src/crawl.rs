//! Crawl mode: start from one URL, follow same-site links, and score each page.
//!
//! Breadth-first, staying on the seed's host (ignoring a leading `www.`),
//! stopping once `max_pages` have been fetched. Fetching uses the same polite
//! settings as a normal scan (delay, timeout, robots.txt).

use crate::config::FetchConfig;
use crate::http::robots_allows;
use crate::keywords::Matcher;
use crate::model::SiteResult;
use crate::verify::extract_text;
use scraper::{Html, Selector};
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

pub async fn crawl(
    seed: &str,
    matcher: &Matcher,
    client: &reqwest::Client,
    cfg: &FetchConfig,
    max_pages: usize,
) -> Vec<SiteResult> {
    let seed_url = match Url::parse(seed) {
        Ok(u) => u,
        Err(e) => {
            return vec![SiteResult {
                url: seed.to_string(),
                fetched: false,
                status: None,
                error: Some(format!("invalid start url: {e}")),
                score: 0,
                excluded: false,
                exclusions: vec![],
                hits: vec![],
            }]
        }
    };
    let seed_host = norm_host(&seed_url);

    let mut queue: VecDeque<Url> = VecDeque::new();
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(key(&seed_url));
    queue.push_back(seed_url);

    let mut results: Vec<SiteResult> = Vec::new();

    while let Some(url) = queue.pop_front() {
        if results.len() >= max_pages {
            break;
        }
        if !results.is_empty() && cfg.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(cfg.delay_ms)).await;
        }

        if cfg.respect_robots && !robots_allows(client, &url, &cfg.user_agent).await {
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

        match client.get(url.clone()).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                match resp.text().await {
                    Ok(body) => {
                        let text = extract_text(&body);
                        let hits = matcher.find(url.as_str(), &text);
                        let exclusions = matcher.exclusions(url.as_str(), &text);
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

                        // Queue same-site links we haven't seen yet.
                        if results.len() + queue.len() < max_pages.saturating_mul(5) {
                            for link in extract_links(&body, &url) {
                                if norm_host(&link) == seed_host && seen.insert(key(&link)) {
                                    queue.push_back(link);
                                }
                            }
                        }
                    }
                    Err(e) => results.push(SiteResult {
                        url: url.to_string(),
                        fetched: false,
                        status: Some(status),
                        error: Some(format!("reading body: {e}")),
                        score: 0,
                        excluded: false,
                        exclusions: vec![],
                        hits: vec![],
                    }),
                }
            }
            Err(e) => results.push(SiteResult {
                url: url.to_string(),
                fetched: false,
                status: None,
                error: Some(format!("request failed: {e}")),
                score: 0,
                excluded: false,
                exclusions: vec![],
                hits: vec![],
            }),
        }
    }

    results
}

/// Pull absolute http(s) links from a page, resolved against its URL.
fn extract_links(html: &str, base: &Url) -> Vec<Url> {
    let doc = Html::parse_document(html);
    let sel = match Selector::parse("a[href]") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let mut out = Vec::new();
    for el in doc.select(&sel) {
        if let Some(href) = el.value().attr("href") {
            if let Ok(mut u) = base.join(href) {
                if u.scheme() == "http" || u.scheme() == "https" {
                    u.set_fragment(None);
                    out.push(u);
                }
            }
        }
    }
    out
}
