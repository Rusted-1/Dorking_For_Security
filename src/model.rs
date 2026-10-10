//! Core data types shared across the tool.

use serde::{Deserialize, Serialize};

/// Where a keyword is matched: against page content, or against the URL itself.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// Match against the page's visible text (default).
    #[default]
    Content,
    /// Match against the candidate URL string (an `inurl:`-style filter).
    Url,
}

impl Target {
    pub fn label(self) -> &'static str {
        match self {
            Target::Content => "content",
            Target::Url => "url",
        }
    }
}

/// A single keyword / fingerprint to look for.
///
/// `phrase` is matched case-insensitively as a substring by default. If
/// `regex` is true, `phrase` is treated as a regular expression instead.
/// `target` chooses whether it is matched against page content or the URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyword {
    pub id: String,
    pub phrase: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub regex: bool,
    /// Match only on whole-word boundaries (good for short, generic terms).
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub target: Target,
    /// Override the category's default scoring weight.
    #[serde(default)]
    pub weight: Option<u32>,
    /// If true, a match disqualifies the page: it is treated as legitimate,
    /// scored 0, and dropped from the ranked results. Checked against both
    /// page content and the URL.
    #[serde(default)]
    pub exclude: bool,
}

/// Top-level shape of the keywords data file (`keywords.json`).
#[derive(Debug, Clone, Deserialize)]
pub struct KeywordFile {
    pub keywords: Vec<Keyword>,
}

/// One keyword that was found, with a bit of surrounding context.
#[derive(Debug, Clone, Serialize)]
pub struct KeywordHit {
    pub keyword_id: String,
    pub category: Option<String>,
    pub matched_in: &'static str,
    pub weight: u32,
    pub snippet: String,
}

/// The outcome of checking a single candidate URL.
#[derive(Debug, Clone, Serialize)]
pub struct SiteResult {
    pub url: String,
    pub fetched: bool,
    pub status: Option<u16>,
    pub error: Option<String>,
    /// Certainty that this is a scam-template page, 1-100 (0 = no hits).
    pub score: u32,
    /// True if an exclusion term matched; the page is treated as legitimate.
    pub excluded: bool,
    /// Which exclusion terms matched (if any).
    pub exclusions: Vec<String>,
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
