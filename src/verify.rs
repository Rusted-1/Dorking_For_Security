//! Fetch candidate pages (politely) and check them for keyword hits.

use crate::config::FetchConfig;
use crate::http::robots_allows;
use crate::keywords::Matcher;
use crate::model::SiteResult;
use scraper::Html;
use std::time::Duration;
use url::Url;

/// Verify a batch of candidate URLs, sequentially and rate-limited.
pub async fn verify_sites(
    urls: &[String],
    matcher: &Matcher,
    client: &reqwest::Client,
    cfg: &FetchConfig,
) -> Vec<SiteResult> {
    let mut results = Vec::with_capacity(urls.len());
    for (i, raw_url) in urls.iter().enumerate() {
        if i > 0 && cfg.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(cfg.delay_ms)).await;
        }
        results.push(verify_one(raw_url, matcher, client, cfg).await);
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
