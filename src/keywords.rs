//! Keyword loading and matching.

use crate::model::{Keyword, KeywordFile, KeywordHit};
use anyhow::{Context, Result};
use regex::Regex;
use std::path::Path;

/// Load keywords from a JSON data file.
pub fn load_keywords(path: &Path) -> Result<Vec<Keyword>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading keywords file {}", path.display()))?;
    let file: KeywordFile = serde_json::from_str(&raw)
        .with_context(|| format!("parsing keywords file {}", path.display()))?;
    Ok(file.keywords)
}

struct Compiled {
    kw: Keyword,
    re: Option<Regex>,
    lower_phrase: String,
}

/// Matches a body of text against a set of keywords.
pub struct Matcher {
    keywords: Vec<Compiled>,
}

impl Matcher {
    pub fn new(keywords: Vec<Keyword>) -> Result<Self> {
        let mut compiled = Vec::with_capacity(keywords.len());
        for kw in keywords {
            let re = if kw.regex {
                Some(
                    Regex::new(&kw.phrase)
                        .with_context(|| format!("invalid regex for keyword '{}'", kw.id))?,
                )
            } else {
                None
            };
            let lower_phrase = kw.phrase.to_lowercase();
            compiled.push(Compiled { kw, re, lower_phrase });
        }
        Ok(Self { keywords: compiled })
    }

    pub fn len(&self) -> usize {
        self.keywords.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keywords.is_empty()
    }

    /// Return every keyword that appears in `text`.
    pub fn find(&self, text: &str) -> Vec<KeywordHit> {
        let lower = text.to_lowercase();
        let mut hits = Vec::new();
        for c in &self.keywords {
            let start = match &c.re {
                Some(re) => re.find(text).map(|m| m.start()),
                None => lower.find(&c.lower_phrase),
            };
            if let Some(start) = start {
                hits.push(KeywordHit {
                    keyword_id: c.kw.id.clone(),
                    category: c.kw.category.clone(),
                    snippet: snippet(text, start, 90),
                });
            }
        }
        hits
    }
}

/// Pull a short, whitespace-collapsed snippet around a byte offset.
fn snippet(text: &str, start: usize, radius: usize) -> String {
    let s = floor_boundary(text, start.saturating_sub(radius));
    let e = ceil_boundary(text, (start + radius).min(text.len()));
    text[s..e].split_whitespace().collect::<Vec<_>>().join(" ")
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kw(id: &str, phrase: &str, regex: bool) -> Keyword {
        Keyword {
            id: id.into(),
            phrase: phrase.into(),
            category: None,
            regex,
        }
    }

    #[test]
    fn matches_case_insensitive_substring() {
        let m = Matcher::new(vec![kw("urgency", "Act Now", false)]).unwrap();
        let hits = m.find("Please ACT NOW before your account closes.");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].keyword_id, "urgency");
        assert!(hits[0].snippet.to_lowercase().contains("act now"));
    }

    #[test]
    fn no_false_positive() {
        let m = Matcher::new(vec![kw("giftcard", "pay with gift cards", false)]).unwrap();
        assert!(m.find("A normal page about nothing in particular.").is_empty());
    }

    #[test]
    fn regex_mode() {
        let m = Matcher::new(vec![kw("acct", r"account #\d{4,}", true)]).unwrap();
        let hits = m.find("your account #12345 is suspended");
        assert_eq!(hits.len(), 1);
    }
}
