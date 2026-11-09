//! Keyword loading and matching.

use crate::model::{Keyword, KeywordFile, KeywordHit, Target};
use crate::score;
use aho_corasick::AhoCorasick;
use anyhow::{Context, Result};
use regex::Regex;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

/// Load keywords from a JSON data file.
pub fn load_keywords(path: &Path) -> Result<Vec<Keyword>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading keywords file {}", path.display()))?;
    let file: KeywordFile = serde_json::from_str(&raw)
        .with_context(|| format!("parsing keywords file {}", path.display()))?;
    let damaged: Vec<&str> = file
        .keywords
        .iter()
        .filter(|k| looks_damaged(&k.phrase))
        .map(|k| k.id.as_str())
        .collect();
    if !damaged.is_empty() {
        eprintln!(
            "Warning: {} keyword(s) in {} contain broken characters and can never match \
             (re-copy them from the source page): {}",
            damaged.len(),
            path.display(),
            damaged.join(", ")
        );
    }
    Ok(file.keywords)
}

/// Text damaged by an encoding mix-up: a replacement character, invisible
/// C1 control characters, the "â€" signature of double-encoded UTF-8, or a
/// UTF-8 lead byte left behind with a lost byte turned into a space.
fn looks_damaged(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    s.contains('\u{FFFD}')
        || s.contains("\u{e2}\u{20ac}")
        || chars.iter().any(|c| ('\u{80}'..='\u{9f}').contains(c))
        || chars.windows(3).any(|w| {
            ('\u{c2}'..='\u{f4}').contains(&w[0]) && w[1] == ' ' && ('\u{80}'..='\u{bf}').contains(&w[2])
        })
}

/// Fold typographic punctuation to its plain form, so a keyword written with
/// curly quotes or dashes matches a page that uses straight ones (and the
/// reverse). Applied to both keywords and the text they're matched against.
fn fold(s: &str) -> Cow<'_, str> {
    if s.is_ascii() {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => out.push('\''),
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => out.push('"'),
            '\u{2010}'..='\u{2015}' | '\u{2212}' => out.push('-'),
            '\u{2026}' => out.push_str("..."),
            '\u{00A0}' | '\u{2007}' | '\u{202F}' => out.push(' '),
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

struct Compiled {
    kw: Keyword,
    /// Regex for regex / whole-word keywords; None for plain phrases, which
    /// are matched by the shared automaton instead.
    re: Option<Regex>,
    weight: u32,
    is_exclude: bool,
}

/// First match position of each plain keyword (by index) in one haystack.
type PlainHits = HashMap<usize, usize>;

impl Compiled {
    /// Where this keyword (index `i`) first appears in `haystack`, if at all.
    fn matches(&self, i: usize, haystack: &str, plain: &PlainHits) -> Option<usize> {
        match &self.re {
            Some(re) => re.find(haystack).map(|m| m.start()),
            None => plain.get(&i).copied(),
        }
    }
}

/// Matches content and URLs against a set of keywords.
///
/// Plain phrases (the vast majority) go into one Aho-Corasick automaton, so a
/// page is scanned once for all of them instead of once per keyword.
pub struct Matcher {
    keywords: Vec<Compiled>,
    /// Automaton over the distinct lowercased plain phrases.
    plain: Option<AhoCorasick>,
    /// For each automaton pattern, the indexes of keywords using that phrase.
    owners: Vec<Vec<usize>>,
}

impl Matcher {
    pub fn new(keywords: Vec<Keyword>) -> Result<Self> {
        let mut compiled = Vec::with_capacity(keywords.len());
        let mut patterns: Vec<String> = Vec::new();
        let mut owners: Vec<Vec<usize>> = Vec::new();
        let mut pattern_of: HashMap<String, usize> = HashMap::new();
        for (idx, kw) in keywords.into_iter().enumerate() {
            let re = if kw.regex {
                Some(
                    Regex::new(&kw.phrase)
                        .with_context(|| format!("invalid regex for keyword '{}'", kw.id))?,
                )
            } else if kw.whole_word {
                let pat = format!(r"(?i)\b{}\b", regex::escape(&fold(&kw.phrase)));
                Some(
                    Regex::new(&pat)
                        .with_context(|| format!("building whole-word regex for '{}'", kw.id))?,
                )
            } else {
                let lower = fold(&kw.phrase).to_lowercase();
                let p = *pattern_of.entry(lower.clone()).or_insert_with(|| {
                    patterns.push(lower);
                    owners.push(Vec::new());
                    patterns.len() - 1
                });
                owners[p].push(idx);
                None
            };
            let weight = kw
                .weight
                .unwrap_or_else(|| score::default_weight(kw.category.as_deref()));
            let is_exclude = kw.exclude;
            compiled.push(Compiled {
                kw,
                re,
                weight,
                is_exclude,
            });
        }
        let plain = if patterns.is_empty() {
            None
        } else {
            Some(AhoCorasick::new(&patterns).context("building the keyword matcher")?)
        };
        Ok(Self {
            keywords: compiled,
            plain,
            owners,
        })
    }

    /// First match position of every plain keyword in a lowercased haystack.
    fn plain_hits(&self, lower: &str) -> PlainHits {
        let mut first = PlainHits::new();
        if let Some(ac) = &self.plain {
            // Overlapping search reports every pattern; for any one pattern
            // the first report is its leftmost occurrence.
            for m in ac.find_overlapping_iter(lower) {
                for &k in &self.owners[m.pattern().as_usize()] {
                    first.entry(k).or_insert(m.start());
                }
            }
        }
        first
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

    /// Check one page in a single pass. Returns the keyword hits (content
    /// keywords against `text`, URL keywords against `url`) and the exclusion
    /// terms found in either; any exclusion means the page is treated as
    /// legitimate.
    pub fn scan(&self, url: &str, text: &str) -> (Vec<KeywordHit>, Vec<String>) {
        let text = fold(text);
        let text = text.as_ref();
        let in_text = self.plain_hits(&text.to_lowercase());
        let in_url = self.plain_hits(&url.to_lowercase());
        let mut hits = Vec::new();
        let mut exclusions = Vec::new();
        for (i, c) in self.keywords.iter().enumerate() {
            if c.is_exclude {
                if c.matches(i, text, &in_text).is_some() || c.matches(i, url, &in_url).is_some() {
                    exclusions.push(c.kw.phrase.clone());
                }
                continue;
            }
            let (haystack, plain) = match c.kw.target {
                Target::Content => (text, &in_text),
                Target::Url => (url, &in_url),
            };
            if let Some(start) = c.matches(i, haystack, plain) {
                hits.push(KeywordHit {
                    keyword_id: c.kw.id.clone(),
                    category: c.kw.category.clone(),
                    matched_in: c.kw.target.label(),
                    weight: c.weight,
                    snippet: snippet(haystack, start, 90),
                });
            }
        }
        (hits, exclusions)
    }

    #[cfg(test)]
    fn find(&self, url: &str, text: &str) -> Vec<KeywordHit> {
        self.scan(url, text).0
    }

    #[cfg(test)]
    fn exclusions(&self, url: &str, text: &str) -> Vec<String> {
        self.scan(url, text).1
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
    fn shared_and_overlapping_phrases_all_match() {
        let m = Matcher::new(vec![
            kw("a", "Investment Platform", false, Target::Content),
            kw("b", "investment platform", false, Target::Content), // same phrase, other id
            kw("c", "Platform", false, Target::Content),             // inside another phrase
            kw("d", "nowhere", false, Target::Content),
        ])
        .unwrap();
        let mut ids: Vec<String> = m
            .find("http://x/", "Our INVESTMENT PLATFORM pays daily")
            .into_iter()
            .map(|h| h.keyword_id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn curly_and_straight_quotes_match_each_other() {
        let m = Matcher::new(vec![
            kw("curly", "judge a firm’s financial integrity", false, Target::Content),
            kw("straight", "it's a \"safe\" bet", false, Target::Content),
        ])
        .unwrap();
        assert_eq!(m.find("http://x/", "how to judge a firm's financial integrity").len(), 1);
        assert_eq!(m.find("http://x/", "It’s a “safe” bet").len(), 1);
    }

    #[test]
    fn spots_double_encoded_text() {
        assert!(looks_damaged("Tesla\u{e2}\u{80}\u{99}s holdings"));
        assert!(looks_damaged("bad \u{FFFD} char"));
        assert!(looks_damaged("friends\u{e2}\u{20ac}\u{201c}meaning"));
        assert!(looks_damaged("\u{e5} \u{ac}\u{8f}"));
        assert!(!looks_damaged("café au lait, à la carte, ±5%"));
        assert!(!looks_damaged("Tesla’s holdings"));
        assert!(!looks_damaged("投资平台"));
    }

    #[test]
    fn shipped_keyword_file_has_no_double_encoding_left() {
        // Guards against re-importing mojibake: allow only the three known
        // phrases (ind-1137, ind-2242, ind-2301) whose original bytes were lost.
        let kws = load_keywords(Path::new("keywords.json")).unwrap();
        let damaged = kws.iter().filter(|k| looks_damaged(&k.phrase)).count();
        assert!(damaged <= 3, "{damaged} damaged keyword phrases");
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
