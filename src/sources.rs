//! Candidate discovery: where the list of URLs to check comes from.

use crate::config::SourceConfig;
use crate::dorks;
use crate::model::Keyword;
use anyhow::{bail, Context, Result};

/// Produce the list of candidate URLs to verify. `keywords` is used to
/// auto-generate dork queries for the search source when none are configured.
pub async fn gather_candidates(
    source: &SourceConfig,
    client: &reqwest::Client,
    keywords: &[Keyword],
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
                // Use configured queries, or auto-build them from the keywords.
                let auto;
                let queries: &[String] = if queries.is_empty() {
                    auto = dorks::build_queries(keywords);
                    &auto
                } else {
                    queries
                };
                if queries.is_empty() {
                    bail!(
                        "no search queries: set `queries` in the config, or add \
                         fingerprint keywords so they can be generated automatically"
                    );
                }
                println!("Running {} dork quer(y/ies) via Google Custom Search…", queries.len());
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

/// Extract result links from one Custom Search JSON API response body.
fn parse_cse_items(body: &serde_json::Value) -> Vec<String> {
    body.get("items")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|it| it.get("link").and_then(|l| l.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Discover candidates via the Google Programmable Search (Custom Search JSON) API.
///
/// NOTE: the live HTTP call reaches googleapis.com and needs an API key + cx,
/// so it is not exercised by the offline tests (the response parsing is). The
/// Custom Search JSON API returns results 10 per page, paged via `start`.
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
            let items = parse_cse_items(&body);
            if items.is_empty() {
                break;
            }
            urls.extend(items);
            start += 10;
        }
    }

    // De-duplicate while preserving order.
    let mut seen = std::collections::HashSet::new();
    urls.retain(|u| seen.insert(u.clone()));
    urls.truncate(max_results);
    Ok(urls)
}

#[cfg(test)]
mod tests {
    use super::parse_cse_items;

    #[test]
    fn parses_links_from_items() {
        let body = serde_json::json!({
            "items": [
                {"title": "A", "link": "https://a.example/"},
                {"title": "B", "link": "https://b.example/register"},
                {"title": "no link here"}
            ]
        });
        let links = parse_cse_items(&body);
        assert_eq!(links, vec!["https://a.example/", "https://b.example/register"]);
    }

    #[test]
    fn empty_when_no_items() {
        assert!(parse_cse_items(&serde_json::json!({"searchInformation": {}})).is_empty());
    }
}
