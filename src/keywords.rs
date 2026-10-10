//! Keyword loading and matching.

use crate::model::{Keyword, KeywordFile, KeywordHit, Target};
use crate::score;
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
    weight: u32,
    is_exclude: bool,
}

impl Compiled {
    /// Does this keyword appear in `haystack`? `lower_haystack` is the
    /// lowercased form, used for the fast substring path.
    fn matches(&self, haystack: &str, lower_haystack: &str) -> Option<usize> {
        match &self.re {
            Some(re) => re.find(haystack).map(|m| m.start()),
            None => lower_haystack.find(&self.lower_phrase),
        }
    }
}

/// Matches content and URLs against a set of keywords.
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
            } else if kw.whole_word {
                let pat = format!(r"(?i)\b{}\b", regex::escape(&kw.phrase));
                Some(
                    Regex::new(&pat)
                        .with_context(|| format!("building whole-word regex for '{}'", kw.id))?,
                )
            } else {
                None
            };
            let lower_phrase = kw.phrase.to_lowercase();
            let weight = kw
                .weight
                .unwrap_or_else(|| score::default_weight(kw.category.as_deref()));
            let is_exclude = kw.exclude;
            compiled.push(Compiled {
                kw,
                re,
                lower_phrase,
                weight,
                is_exclude,
            });
        }
        Ok(Self { keywords: compiled })
    }

    pub fn len(&self) -> usize {
        self.keywords.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keywords.is_empty()
    }

    /// Keyword counts: (content, url, exclusion).
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut content = 0;
        let mut url = 0;
        let mut exclude = 0;
        for c in &self.keywords {
            if c.is_exclude {
                exclude += 1;
            } else if c.kw.target == Target::Url {
                url += 1;
            } else {
                content += 1;
            }
        }
        (content, url, exclude)
    }

    /// Return the exclusion terms that appear in the content or the URL.
    /// A non-empty result means the page should be treated as legitimate.
    pub fn exclusions(&self, url: &str, text: &str) -> Vec<String> {
        let lower_text = text.to_lowercase();
        let lower_url = url.to_lowercase();
        let mut out = Vec::new();
        for c in &self.keywords {
            if !c.is_exclude {
                continue;
            }
            if c.matches(text, &lower_text).is_some() || c.matches(url, &lower_url).is_some() {
                out.push(c.kw.phrase.clone());
            }
        }
        out
    }

    /// Find every keyword hit, checking content keywords against `text` and
    /// URL keywords against `url`.
    pub fn find(&self, url: &str, text: &str) -> Vec<KeywordHit> {
        let lower_text = text.to_lowercase();
        let lower_url = url.to_lowercase();
        let mut hits = Vec::new();
        for c in &self.keywords {
            if c.is_exclude {
                continue;
            }
            let (haystack, lower_haystack) = match c.kw.target {
                Target::Content => (text, &lower_text),
                Target::Url => (url, &lower_url),
            };
            if let Some(start) = c.matches(haystack, lower_haystack) {
                hits.push(KeywordHit {
                    keyword_id: c.kw.id.clone(),
                    category: c.kw.category.clone(),
                    matched_in: c.kw.target.label(),
                    weight: c.weight,
                    snippet: snippet(haystack, start, 90),
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

    fn kw(id: &str, phrase: &str, regex: bool, target: Target) -> Keyword {
        Keyword {
            id: id.into(),
            phrase: phrase.into(),
            category: None,
            regex,
            whole_word: false,
            target,
            weight: None,
            exclude: false,
        }
    }

    fn word_kw(id: &str, phrase: &str) -> Keyword {
        Keyword {
            id: id.into(),
            phrase: phrase.into(),
            category: None,
            regex: false,
            whole_word: true,
            target: Target::Content,
            weight: None,
            exclude: false,
        }
    }

    fn exclude_kw(id: &str, phrase: &str) -> Keyword {
        Keyword {
            exclude: true,
            ..word_kw(id, phrase)
        }
    }

    #[test]
    fn whole_word_avoids_substring_false_positives() {
        let m = Matcher::new(vec![word_kw("coin", "coin")]).unwrap();
        // Must NOT match inside "bitcoin"; MUST match the standalone word.
        assert!(m.find("http://x/", "I love bitcoin trading").is_empty());
        assert_eq!(m.find("http://x/", "buy a coin today").len(), 1);
    }

    #[test]
    fn exclusions_are_detected_and_not_scored() {
        let m = Matcher::new(vec![
            word_kw("crypto", "crypto"),
            exclude_kw("bank", "bank"),
        ])
        .unwrap();
        // The exclusion keyword must not appear as a scoring hit...
        let hits = m.find("http://x/", "crypto services from your bank");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].keyword_id, "crypto");
        // ...but must be reported by exclusions(), from content or URL.
        assert_eq!(m.exclusions("http://x/", "crypto services from your bank"), vec!["bank"]);
        assert_eq!(m.exclusions("http://bank.example/", "clean body"), vec!["bank"]);
        assert!(m.exclusions("http://x/", "no legit terms here").is_empty());
    }

    #[test]
    fn matches_case_insensitive_substring() {
        let m = Matcher::new(vec![kw("urgency", "Act Now", false, Target::Content)]).unwrap();
        let hits = m.find("http://x/", "Please ACT NOW before your account closes.");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].keyword_id, "urgency");
        assert_eq!(hits[0].matched_in, "content");
    }

    #[test]
    fn no_false_positive() {
        let m = Matcher::new(vec![kw("giftcard", "pay with gift cards", false, Target::Content)]).unwrap();
        assert!(m.find("http://x/", "A normal page about nothing.").is_empty());
    }

    #[test]
    fn regex_mode() {
        let m = Matcher::new(vec![kw("acct", r"account #\d{4,}", true, Target::Content)]).unwrap();
        assert_eq!(m.find("http://x/", "your account #12345 is suspended").len(), 1);
    }

    #[test]
    fn url_target_matches_url_not_content() {
        let m = Matcher::new(vec![kw("reg", "/user/register", false, Target::Url)]).unwrap();
        let hit = m.find("https://scam.example/user/register", "nothing relevant here");
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].matched_in, "url");
        // A content keyword must not match text that only appears in the URL.
        let m2 = Matcher::new(vec![kw("reg", "register", false, Target::Content)]).unwrap();
        assert!(m2.find("https://scam.example/register", "clean body").is_empty());
    }
}
