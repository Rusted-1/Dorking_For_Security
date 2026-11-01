//! Run configuration, loaded from a TOML file.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Environment variable that supplies (or overrides) the Serper API key, so a
/// key never has to be written into a file at all.
pub const SERPER_KEY_ENV: &str = "SERPER_API_KEY";

/// Quote a string as a TOML basic string. JSON string escapes are a subset of
/// TOML's, so serde_json does it without pulling in a TOML writer.
fn toml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Write a config.toml. A non-empty `api_key` produces a search_api (auto
/// discovery) source; otherwise a url_list source. Shared by the `setup`
/// wizard and the control panel's setup form.
pub fn write_config_file(
    path: &Path,
    provider: &str,
    api_key: &str,
    cx: &str,
    delay_ms: u64,
    output_dir: &str,
) -> Result<()> {
    let source = if !api_key.is_empty() {
        // `cx` is only used by google_cse; harmless (empty) for other providers.
        format!(
            "[source]\nkind = \"search_api\"\nprovider = {provider}\n\
             api_key = {api_key}\ncx = {cx}\n\
             # Most new sites to queue per discovery run.\nmax_results = 50\n\
             # Results requested per query (10 = cheapest on Serper credits).\nresults_per_query = 10\n\
             # Credit budget: at most this many queries per run. Queries rotate,\n\
             # least-recently-run first, so repeated runs cover the whole list.\nmax_queries_per_run = 25\n\
             # Don't re-scan a site scanned within this many days (0 = always re-scan).\nrescan_after_days = 30\n\
             # Extra domains to never queue (a built-in list already skips big\n\
             # legitimate sites like wikipedia.org, reddit.com, .gov, .edu).\nskip_domains = []\n\
             # queries empty = auto-build dorks from keywords.json\nqueries = []\n",
            provider = toml_str(provider),
            api_key = toml_str(api_key),
            cx = toml_str(cx),
        )
    } else {
        "[source]\nkind = \"url_list\"\npath = \"candidates.txt\"\n".to_string()
    };
    let contents = format!(
        "# Written by Dorker setup.\n\
         keywords_file = \"keywords.json\"\n\
         output_dir = {output_dir}\n\n\
         [fetch]\n\
         delay_ms = {delay_ms}\n\
         timeout_secs = 20\n\
         user_agent = \"Dorker/0.1 (+anti-fraud research)\"\n\
         respect_robots = true\n\n\
         {source}",
        output_dir = toml_str(output_dir),
    );
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub keywords_file: String,
    #[serde(default = "default_output_dir")]
    pub output_dir: String,
    #[serde(default)]
    pub fetch: FetchConfig,
    pub source: SourceConfig,
}

fn default_output_dir() -> String {
    "out".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct FetchConfig {
    /// Delay between requests, in milliseconds (politeness / rate limiting).
    #[serde(default = "d_delay")]
    pub delay_ms: u64,
    #[serde(default = "d_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "d_user_agent")]
    pub user_agent: String,
    #[serde(default = "d_true")]
    pub respect_robots: bool,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            delay_ms: d_delay(),
            timeout_secs: d_timeout(),
            user_agent: d_user_agent(),
            respect_robots: d_true(),
        }
    }
}

fn d_delay() -> u64 {
    1500
}
fn d_timeout() -> u64 {
    20
}
fn d_true() -> bool {
    true
}
fn d_user_agent() -> String {
    "Dorker/0.1 (+anti-fraud research; configure a contact address)".to_string()
}

/// Where candidate URLs come from.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceConfig {
    /// Read candidate URLs from a local file, one per line.
    UrlList { path: String },
    /// Discover candidates through an official search API (see sources::search_api).
    SearchApi {
        /// "serper" (recommended) or "google_cse"
        provider: String,
        #[serde(default)]
        api_key: String,
        /// Google Programmable Search engine id (cx).
        #[serde(default)]
        cx: String,
        /// Most new candidate sites to keep per discovery run.
        #[serde(default = "d_max_results")]
        max_results: usize,
        /// Results requested per query (Serper `num`, 10..=100).
        #[serde(default = "d_results_per_query")]
        results_per_query: usize,
        /// Credit budget: most queries to spend per run (0 = no limit).
        #[serde(default = "d_max_queries_per_run")]
        max_queries_per_run: usize,
        /// Skip sites scanned within this many days (0 = always re-scan).
        #[serde(default = "d_rescan_after_days")]
        rescan_after_days: u64,
        /// Extra domains never to queue, on top of the built-in skip list.
        #[serde(default)]
        skip_domains: Vec<String>,
        /// The dork queries to run.
        #[serde(default)]
        queries: Vec<String>,
    },
}

fn d_max_results() -> usize {
    50
}
fn d_results_per_query() -> usize {
    10
}
fn d_max_queries_per_run() -> usize {
    25
}
fn d_rescan_after_days() -> u64 {
    30
}

impl SourceConfig {
    /// A Serper discovery source with every setting at its default.
    fn default_serper(api_key: String) -> Self {
        SourceConfig::SearchApi {
            provider: "serper".to_string(),
            api_key,
            cx: String::new(),
            max_results: d_max_results(),
            results_per_query: d_results_per_query(),
            max_queries_per_run: d_max_queries_per_run(),
            rescan_after_days: d_rescan_after_days(),
            skip_domains: vec![],
            queries: vec![],
        }
    }

    /// True when this is a search source with a usable key.
    pub fn discovery_ready(&self) -> bool {
        matches!(self, SourceConfig::SearchApi { api_key, .. } if !api_key.is_empty())
    }
}

fn env_serper_key() -> Option<String> {
    std::env::var(SERPER_KEY_ENV)
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

/// Load the config, applying the `SERPER_API_KEY` environment variable.
///
/// When the file is missing but the variable is set, a default Serper
/// discovery config is used, so `dorker auto` works with zero setup. When the
/// file uses provider "serper", a set variable overrides its `api_key`.
pub fn load_config(path: &Path) -> Result<Config> {
    let env_key = env_serper_key();
    if !path.exists() {
        if let Some(key) = env_key {
            return Ok(Config {
                keywords_file: "keywords.json".to_string(),
                output_dir: default_output_dir(),
                fetch: FetchConfig::default(),
                source: SourceConfig::default_serper(key),
            });
        }
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))?;
    let mut cfg: Config =
        toml::from_str(&raw).with_context(|| format!("parsing config file {}", path.display()))?;
    if let (Some(key), SourceConfig::SearchApi { provider, api_key, .. }) =
        (env_key, &mut cfg.source)
    {
        if provider == "serper" {
            *api_key = key;
        }
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_config_round_trips_and_escapes() {
        let dir = std::env::temp_dir().join(format!("dorker-cfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        write_config_file(&path, "serper", "ab\"c\\d", "", 900, "my out").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let cfg: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg.output_dir, "my out");
        assert_eq!(cfg.fetch.delay_ms, 900);
        match cfg.source {
            SourceConfig::SearchApi { api_key, results_per_query, max_queries_per_run, .. } => {
                assert_eq!(api_key, "ab\"c\\d");
                assert_eq!(results_per_query, 10);
                assert_eq!(max_queries_per_run, 25);
            }
            _ => panic!("expected a search_api source"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_configs_still_parse_with_defaults() {
        let raw = "keywords_file = \"keywords.json\"\n[source]\nkind = \"search_api\"\n\
                   provider = \"serper\"\napi_key = \"k\"\nmax_results = 5\nqueries = []\n";
        let cfg: Config = toml::from_str(raw).unwrap();
        match cfg.source {
            SourceConfig::SearchApi { max_results, rescan_after_days, skip_domains, .. } => {
                assert_eq!(max_results, 5);
                assert_eq!(rescan_after_days, 30);
                assert!(skip_domains.is_empty());
            }
            _ => panic!("expected a search_api source"),
        }
    }
}
