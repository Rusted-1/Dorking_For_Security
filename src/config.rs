//! Run configuration, loaded from a TOML file.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Write a config.toml. A non-empty `api_key` produces a search_api (auto
/// discovery) source; otherwise a url_list source. Shared by the `setup`
/// wizard and the control panel's setup form.
pub fn write_config_file(
    path: &Path,
    api_key: &str,
    cx: &str,
    delay_ms: u64,
    output_dir: &str,
) -> Result<()> {
    let source = if !api_key.is_empty() {
        format!(
            "[source]\nkind = \"search_api\"\nprovider = \"google_cse\"\n\
             api_key = \"{api_key}\"\ncx = \"{cx}\"\nmax_results = 50\n\
             # queries empty = auto-build dorks from keywords.json\nqueries = []\n"
        )
    } else {
        "[source]\nkind = \"url_list\"\npath = \"candidates.txt\"\n".to_string()
    };
    let contents = format!(
        "# Written by Dorker setup.\n\
         keywords_file = \"keywords.json\"\n\
         output_dir = \"{output_dir}\"\n\n\
         [fetch]\n\
         delay_ms = {delay_ms}\n\
         timeout_secs = 20\n\
         user_agent = \"Dorker/0.1 (+anti-fraud research)\"\n\
         respect_robots = true\n\n\
         {source}"
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
        /// e.g. "google_cse"
        provider: String,
        #[serde(default)]
        api_key: String,
        /// Google Programmable Search engine id (cx).
        #[serde(default)]
        cx: String,
        #[serde(default = "d_max_results")]
        max_results: usize,
        /// The dork queries to run.
        #[serde(default)]
        queries: Vec<String>,
    },
}

fn d_max_results() -> usize {
    50
}

pub fn load_config(path: &Path) -> Result<Config> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))?;
    let cfg: Config =
        toml::from_str(&raw).with_context(|| format!("parsing config file {}", path.display()))?;
    Ok(cfg)
}
