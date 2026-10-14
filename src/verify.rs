//! Fetch candidate pages (politely) and check them for keyword hits.

use crate::config::FetchConfig;
use crate::coord::{self, Coordinator};
use crate::http::robots_allows;
use crate::keywords::Matcher;
use crate::model::SiteResult;
use scraper::Html;
use std::collections::HashSet;
use std::time::Duration;
use url::Url;

/// Verify a batch of candidate URLs, sequentially and rate-limited.
pub async fn verify_sites(
    urls: &[String],
    matcher: &Matcher,
    client: &reqwest::Client,
    cfg: &FetchConfig,
    coord: Option<&Coordinator>,
) -> Vec<SiteResult> {
    let mut results = Vec::with_capacity(urls.len());
    let mut scanned = 0usize;
    // Hosts this instance owns, so later URLs on the same site aren't skipped.
    let mut owned_hosts: HashSet<String> = HashSet::new();
    for raw_url in urls {
        if let Some(c) = coord {
            if c.stop_requested() {
                eprintln!("  stop requested — halting.");
                break;
            }
            // Site-level claim: stay off sites another instance owns.
            if let Some(host) = coord::host_key(raw_url) {
                if !owned_hosts.contains(&host) {
                    if c.claim_site(&host) {
                        owned_hosts.insert(host);
                    } else {
                        eprintln!("  skip — site '{host}' is handled by another instance");
                        continue;
                    }
                }
            }
            if !c.claim_url(raw_url) {
                continue;
            }
        }
        if scanned > 0 && cfg.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(cfg.delay_ms)).await;
        }
        results.push(verify_one(raw_url, matcher, client, cfg).await);
        scanned += 1;
    }
    results
}

fn failure(url: &str, status: Option<u16>, error: String) -> SiteResult {
    SiteResult {
        url: url.to_string(),
        fetched: false,
        status,
        error: Some(error),
        score: 0,
        excluded: false,
        exclusions: vec![],
        hits: vec![],
    }
}

async fn verify_one(
    raw_url: &str,
    matcher: &Matcher,
    client: &reqwest::Client,
    cfg: &FetchConfig,
) -> SiteResult {
    let parsed = match Url::parse(raw_url) {
        Ok(u) => u,
        Err(e) => return failure(raw_url, None, format!("invalid url: {e}")),
    };

    if cfg.respect_robots && !robots_allows(client, &parsed, &cfg.user_agent).await {
        return failure(raw_url, None, "skipped: disallowed by robots.txt".to_string());
    }

    match client.get(parsed).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            match resp.text().await {
                Ok(body) => {
                    let text = extract_text(&body);
                    let hits = matcher.find(raw_url, &text);
                    let exclusions = matcher.exclusions(raw_url, &text);
                    SiteResult {
                        url: raw_url.to_string(),
                        fetched: true,
                        status: Some(status),
                        error: None,
                        score: 0, // filled in by report::build_report
                        excluded: !exclusions.is_empty(),
                        exclusions,
                        hits,
                    }
                }
                Err(e) => failure(raw_url, Some(status), format!("reading body: {e}")),
            }
        }
        Err(e) => failure(raw_url, None, format!("request failed: {e}")),
    }
}

/// Extract visible text from an HTML document (also works on plain text).
pub fn extract_text(html: &str) -> String {
    let doc = Html::parse_document(html);
    doc.root_element().text().collect::<Vec<_>>().join(" ")
}
