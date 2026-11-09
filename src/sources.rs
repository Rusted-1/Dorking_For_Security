//! Candidate discovery: where the list of URLs to check comes from.

use crate::config::SourceConfig;
use crate::coord;
use crate::dorks;
use crate::model::Keyword;
use crate::state::State;
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::time::Duration;
use url::Url;

/// Domains never worth queueing from search results: big legitimate sites,
/// news/reference, and scam-*reporting* sites that quote the same phrases.
/// An entry matches the domain and all its subdomains; a bare TLD ("gov")
/// matches every host under it. Extend per-run with `skip_domains`.
const BUILTIN_SKIP_DOMAINS: &[&str] = &[
    // government / education
    "gov", "mil", "edu", "gov.uk", "gc.ca", "gov.au", "europa.eu",
    // regulators and scam-reporting / reputation sites
    "finra.org", "fca.org.uk", "bbb.org", "scamadviser.com", "scam-detector.com",
    "trustpilot.com", "sitejabber.com", "reviews.io", "scamwatch.gov.au",
    // reference, news and finance media
    "wikipedia.org", "wikimedia.org", "investopedia.com", "forbes.com", "bloomberg.com",
    "reuters.com", "cnbc.com", "nytimes.com", "wsj.com", "bbc.co.uk", "bbc.com",
    "theguardian.com", "yahoo.com", "marketwatch.com", "fool.com",
    // social, forums and video
    "facebook.com", "instagram.com", "twitter.com", "x.com", "linkedin.com",
    "reddit.com", "quora.com", "medium.com", "youtube.com", "tiktok.com",
    "pinterest.com", "t.me", "github.com", "stackexchange.com",
    // large platforms and real exchanges
    "google.com", "apple.com", "microsoft.com", "amazon.com", "binance.com",
    "coinbase.com", "kraken.com", "coinmarketcap.com", "coingecko.com",
    "tradingview.com", "etoro.com",
];

/// Produce the list of candidate URLs to verify. `keywords` is used to
/// auto-generate dork queries for the search source when none are configured.
/// `state` (when given) rotates queries across runs and skips sites already
/// scanned or reviewed.
pub async fn gather_candidates(
    source: &SourceConfig,
    client: &reqwest::Client,
    keywords: &[Keyword],
    state: Option<&State>,
) -> Result<Vec<String>> {
    match source {
        SourceConfig::UrlList { path } => from_url_list(path),
        SourceConfig::SearchApi {
            provider,
            api_key,
            cx,
            max_results,
            results_per_query,
            max_queries_per_run,
            rescan_after_days,
            skip_domains,
            serper_endpoint,
            queries,
        } => {
            // Use configured queries, or auto-build them from the keywords.
            let all: Vec<String> = if queries.is_empty() {
                dorks::build_queries(keywords)
            } else {
                queries.clone()
            };
            if all.is_empty() {
                bail!(
                    "no search queries: set `queries` in the config, or add \
                     fingerprint keywords so they can be generated automatically"
                );
            }
            let picked = match state {
                Some(s) => s.pick_queries(&all, *max_queries_per_run),
                None => {
                    let n = if *max_queries_per_run == 0 { all.len() } else { *max_queries_per_run };
                    all.iter().take(n).cloned().collect()
                }
            };

            // Sites to leave alone: recently scanned, or already reviewed.
            let mut known: HashSet<String> = HashSet::new();
            if let Some(s) = state {
                known.extend(s.recently_seen(*rescan_after_days));
                known.extend(s.reviews().into_keys());
            }
            let mut collector = Collector::new(skip_domains, known, *max_results);

            match provider.as_str() {
                "google_cse" => {
                    println!(
                        "Running {} of {} dork quer(y/ies) via Google Custom Search…",
                        picked.len(),
                        all.len()
                    );
                    google_cse(client, api_key, cx, &picked, &mut collector, state).await?;
                }
                "serper" => {
                    println!(
                        "Running up to {} of {} dork quer(y/ies) via Serper (least-recently-run first)…",
                        picked.len(),
                        all.len()
                    );
                    if *serper_endpoint != crate::config::d_serper_endpoint() {
                        println!("  (using test endpoint {serper_endpoint})");
                    }
                    let api = SerperApi { endpoint: serper_endpoint, key: api_key };
                    serper(client, &api, &picked, *results_per_query, &mut collector, state).await?;
                }
                other => bail!(
                    "unknown search provider '{other}'. Supported: serper, google_cse. \
                     (Add your provider in src/sources.rs.)"
                ),
            }
            println!("{}", collector.summary());
            Ok(collector.kept)
        }
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

/// Does `host` fall under `domain` (the domain itself or any subdomain)?
fn domain_matches(host: &str, domain: &str) -> bool {
    let d = domain
        .trim()
        .trim_start_matches("*.")
        .trim_start_matches('.')
        .to_lowercase();
    !d.is_empty() && (host == d || host.ends_with(&format!(".{d}")))
}

/// Filters search results down to new, distinct, worth-scanning sites.
struct Collector {
    skip_domains: Vec<String>,
    known: HashSet<String>,
    hosts: HashSet<String>,
    max: usize,
    kept: Vec<String>,
    invalid: usize,
    skipped_domain: usize,
    already_known: usize,
    duplicate: usize,
}

impl Collector {
    fn new(extra_skip: &[String], known: HashSet<String>, max: usize) -> Self {
        let mut skip_domains: Vec<String> =
            BUILTIN_SKIP_DOMAINS.iter().map(|d| d.to_string()).collect();
        skip_domains.extend(extra_skip.iter().cloned());
        Self {
            skip_domains,
            known,
            hosts: HashSet::new(),
            max,
            kept: Vec::new(),
            invalid: 0,
            skipped_domain: 0,
            already_known: 0,
            duplicate: 0,
        }
    }

    fn full(&self) -> bool {
        self.max > 0 && self.kept.len() >= self.max
    }

    /// Consider one search result; keep it if it's a new, scannable site.
    fn add(&mut self, raw: &str) {
        if self.full() {
            return;
        }
        let ok_scheme = Url::parse(raw)
            .map(|u| u.scheme() == "http" || u.scheme() == "https")
            .unwrap_or(false);
        let Some(host) = coord::host_key(raw).filter(|_| ok_scheme) else {
            self.invalid += 1;
            return;
        };
        let bare = host.split(':').next().unwrap_or(&host);
        if self.skip_domains.iter().any(|d| domain_matches(bare, d)) {
            self.skipped_domain += 1;
        } else if self.known.contains(&host) {
            self.already_known += 1;
        } else if !self.hosts.insert(host) {
            self.duplicate += 1;
        } else {
            self.kept.push(raw.to_string());
        }
    }

    fn summary(&self) -> String {
        format!(
            "Kept {} new site(s). Filtered out: {} big/legitimate domain(s), {} already \
             scanned or reviewed, {} duplicate(s) of a kept site, {} invalid.",
            self.kept.len(),
            self.skipped_domain,
            self.already_known,
            self.duplicate,
            self.invalid
        )
    }
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
    collector: &mut Collector,
    state: Option<&State>,
) -> Result<()> {
    if api_key.is_empty() || cx.is_empty() {
        bail!("google_cse source needs both `api_key` and `cx` set in the config");
    }

    for query in queries {
        let mut start = 1usize; // Custom Search is 1-indexed
        while !collector.full() && start <= 91 {
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
            for u in &items {
                collector.add(u);
            }
            start += 10;
        }
        if let Some(s) = state {
            s.mark_query_run(query);
        }
        if collector.full() {
            break;
        }
    }
    Ok(())
}

/// Extract result links from one Serper (google.serper.dev) response body.
fn parse_serper_items(body: &serde_json::Value) -> Vec<String> {
    body.get("organic")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|it| it.get("link").and_then(|l| l.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Why a single Serper query failed.
enum QueryError {
    /// The key or account is the problem; further queries would fail too.
    Fatal(String),
    /// Only this query failed (after retries); move on to the next one.
    Skip(String),
}

/// Serper's `num` accepts multiples of 10 up to 100.
fn serper_num(results_per_query: usize) -> usize {
    (results_per_query.clamp(10, 100) + 9) / 10 * 10
}

/// Where and how to call Serper.
struct SerperApi<'a> {
    endpoint: &'a str,
    key: &'a str,
}

/// Run one Serper query, retrying rate limits and server errors with backoff.
async fn serper_query(
    client: &reqwest::Client,
    api: &SerperApi<'_>,
    query: &str,
    num: usize,
) -> std::result::Result<Vec<String>, QueryError> {
    const TRIES: u32 = 3;
    let mut wait = Duration::from_secs(2);
    let mut attempt = 1;
    loop {
        let retry_reason = match client
            .post(api.endpoint)
            .header("X-API-KEY", api.key)
            .json(&serde_json::json!({ "q": query, "num": num }))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                return match resp.json::<serde_json::Value>().await {
                    Ok(body) => Ok(parse_serper_items(&body)),
                    Err(e) => Err(QueryError::Skip(format!("unreadable response: {e}"))),
                };
            }
            Ok(resp) => {
                let status = resp.status();
                let detail: String = resp.text().await.unwrap_or_default().chars().take(200).collect();
                let lower = detail.to_lowercase();
                match status.as_u16() {
                    401 | 403 => {
                        return Err(QueryError::Fatal(format!(
                            "Serper rejected the API key (HTTP {status}). Check the key at \
                             https://serper.dev/api-key. Serper said: {detail}"
                        )))
                    }
                    400 | 402 if lower.contains("credit") || lower.contains("balance") => {
                        return Err(QueryError::Fatal(format!(
                            "Serper account is out of credits (HTTP {status}). Serper said: {detail}"
                        )))
                    }
                    429 | 500..=599 => format!("HTTP {status}"),
                    _ => return Err(QueryError::Skip(format!("HTTP {status}: {detail}"))),
                }
            }
            Err(e) => format!("request failed: {e}"),
        };
        if attempt >= TRIES {
            return Err(QueryError::Skip(format!("{retry_reason} (gave up after {TRIES} tries)")));
        }
        eprintln!("  Serper: {retry_reason} — retrying in {}s…", wait.as_secs());
        tokio::time::sleep(wait).await;
        wait *= 2;
        attempt += 1;
    }
}

/// Discover candidates via Serper (real Google results over a simple key).
///
/// POST https://google.serper.dev/search with header `X-API-KEY`, one request
/// per query. Stops early once the collector is full, so credits aren't spent
/// on results that would be thrown away. A failed query is skipped (results
/// so far are kept); a rejected key stops discovery. Reaches google.serper.dev,
/// so the live call is not exercised by offline tests (the parsing is).
async fn serper(
    client: &reqwest::Client,
    api: &SerperApi<'_>,
    queries: &[String],
    results_per_query: usize,
    collector: &mut Collector,
    state: Option<&State>,
) -> Result<()> {
    if api.key.is_empty() {
        bail!(
            "serper source needs an API key: set `api_key` in the config or the {} \
             environment variable (free key at https://serper.dev)",
            crate::config::SERPER_KEY_ENV
        );
    }
    let num = serper_num(results_per_query);
    let (mut spent, mut failed) = (0usize, 0usize);
    for query in queries {
        if collector.full() {
            println!("Reached max_results — saving the remaining queries for a later run.");
            break;
        }
        spent += 1;
        match serper_query(client, api, query, num).await {
            Ok(links) => {
                for u in &links {
                    collector.add(u);
                }
                if let Some(s) = state {
                    s.mark_query_run(query);
                }
            }
            Err(QueryError::Skip(why)) => {
                failed += 1;
                eprintln!("  skipped query {query}: {why}");
            }
            Err(QueryError::Fatal(why)) => {
                if collector.kept.is_empty() {
                    bail!(why);
                }
                eprintln!("  {why}\n  Stopping discovery; keeping what was found so far.");
                break;
            }
        }
    }
    println!("Used {spent} Serper quer(y/ies) ({failed} failed).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_links_from_serper() {
        let body = serde_json::json!({
            "organic": [
                {"title": "A", "link": "https://a.example/"},
                {"title": "B", "link": "https://b.example/register"},
                {"title": "no link"}
            ]
        });
        assert_eq!(
            parse_serper_items(&body),
            vec!["https://a.example/", "https://b.example/register"]
        );
    }

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

    #[test]
    fn serper_num_is_a_multiple_of_ten() {
        assert_eq!(serper_num(0), 10);
        assert_eq!(serper_num(10), 10);
        assert_eq!(serper_num(25), 30);
        assert_eq!(serper_num(500), 100);
    }

    #[test]
    fn domain_matching_covers_subdomains_and_tlds() {
        assert!(domain_matches("en.wikipedia.org", "wikipedia.org"));
        assert!(domain_matches("wikipedia.org", "wikipedia.org"));
        assert!(domain_matches("www.sec.gov", "gov"));
        assert!(domain_matches("a.b.example", "*.b.example"));
        assert!(!domain_matches("notwikipedia.org", "wikipedia.org"));
        assert!(!domain_matches("x.com.scam.example", "x.com"));
    }

    #[test]
    fn collector_filters_and_dedupes() {
        let known: HashSet<String> = ["seen.example".to_string()].into_iter().collect();
        let mut c = Collector::new(&["extra.example".to_string()], known, 10);
        for u in [
            "https://scam-one.example/register",
            "https://www.scam-one.example/login", // same site as above
            "https://en.wikipedia.org/wiki/Fraud", // built-in skip
            "https://shop.extra.example/",        // configured skip
            "https://seen.example/",              // scanned recently
            "ftp://files.example/",               // not http(s)
            "https://scam-two.example/",
        ] {
            c.add(u);
        }
        assert_eq!(
            c.kept,
            vec!["https://scam-one.example/register", "https://scam-two.example/"]
        );
        assert_eq!((c.duplicate, c.skipped_domain, c.already_known, c.invalid), (1, 2, 1, 1));
    }

    #[test]
    fn collector_stops_at_max() {
        let mut c = Collector::new(&[], HashSet::new(), 1);
        c.add("https://a.example/");
        assert!(c.full());
        c.add("https://b.example/");
        assert_eq!(c.kept.len(), 1);
    }
}
