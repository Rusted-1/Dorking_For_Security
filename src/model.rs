//! Core data types shared across the tool.

use serde::{Deserialize, Serialize};

/// A single keyword / fingerprint to look for.
///
/// `phrase` is matched case-insensitively as a substring by default. If
/// `regex` is true, `phrase` is treated as a regular expression instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyword {
    pub id: String,
    pub phrase: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub regex: bool,
}

/// Top-level shape of the keywords data file (`keywords.json`).
#[derive(Debug, Clone, Deserialize)]
pub struct KeywordFile {
    pub keywords: Vec<Keyword>,
}

/// One keyword that was found on a page, with a bit of surrounding context.
#[derive(Debug, Clone, Serialize)]
pub struct KeywordHit {
    pub keyword_id: String,
    pub category: Option<String>,
    pub snippet: String,
}

/// The outcome of checking a single candidate URL.
#[derive(Debug, Clone, Serialize)]
pub struct SiteResult {
    pub url: String,
    pub fetched: bool,
    pub status: Option<u16>,
    pub error: Option<String>,
    pub hits: Vec<KeywordHit>,
}

/// The full run report.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub generated_at: String,
    pub total_candidates: usize,
    pub sites_with_hits: usize,
    pub results: Vec<SiteResult>,
}
