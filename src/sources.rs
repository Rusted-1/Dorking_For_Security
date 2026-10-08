//! Candidate discovery: where the list of URLs to check comes from.

use crate::config::SourceConfig;
use anyhow::{bail, Context, Result};

/// Produce the list of candidate URLs to verify.
pub async fn gather_candidates(
    source: &SourceConfig,
    client: &reqwest::Client,
) -> Result<Vec<String>> {
    match source {
        SourceConfig::UrlList { path } => from_url_list(path),
        SourceConfig::SearchApi {
            provider,
            api_key,
            cx,
            max_results,
            queries,
        } => match provider.as_str() {
            "google_cse" => {
                google_cse(client, api_key, cx, queries, *max_results).await
            }
            other => bail!(
                "unknown search provider '{other}'. Supported: google_cse. \
                 (Add your provider in src/sources.rs.)"
            ),
        },
    }
}

/// Read candidate URLs from a local file, one per line. Blank lines and
/// lines starting with '#' are ignored.
fn from_url_list(path: &str) -> Result<Vec<String>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading url list {path}"))?;
    let urls = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect();
    Ok(urls)
}

/// Discover candidates via the Google Programmable Search (Custom Search JSON) API.
///
/// NOTE: this requires an API key + search-engine id (cx), and reaches
/// googleapis.com, so it is not exercised by the offline tests. The request
/// shape follows the Custom Search JSON API: results come back 10 per page,
/// paged with the `start` parameter.
async fn google_cse(
    client: &reqwest::Client,
    api_key: &str,
    cx: &str,
    queries: &[String],
    max_results: usize,
) -> Result<Vec<String>> {
    if api_key.is_empty() || cx.is_empty() {
        bail!("google_cse source needs both `api_key` and `cx` set in the config");
    }
    if queries.is_empty() {
        bail!("google_cse source needs at least one entry in `queries`");
    }

    let mut urls = Vec::new();
    for query in queries {
        let mut start = 1usize; // Custom Search is 1-indexed
        while urls.len() < max_results && start <= 91 {
            let resp = client
                .get("https://www.googleapis.com/customsearch/v1")
                .query(&[
                    ("key", api_key),
                    ("cx", cx),
                    ("q", query.as_str()),
                    ("start", &start.to_string()),
                ])
                .send()
                .await
                .context("calling Google Custom Search API")?;

            if !resp.status().is_success() {
                bail!("Custom Search API returned HTTP {}", resp.status());
            }

            let body: serde_json::Value = resp.json().await.context("parsing search response")?;
            let items = body
                .get("items")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            if items.is_empty() {
                break;
            }
            for item in items {
                if let Some(link) = item.get("link").and_then(|v| v.as_str()) {
                    urls.push(link.to_string());
                }
            }
            start += 10;
        }
    }

    // De-duplicate while preserving order.
    let mut seen = std::collections::HashSet::new();
    urls.retain(|u| seen.insert(u.clone()));
    urls.truncate(max_results);
    Ok(urls)
}
