//! Build search-engine dork queries automatically from the keyword lists.
//!
//! Distinctive fingerprint phrases become quoted-phrase queries; the most
//! distinctive ones (taglines) are also paired with each `inurl:` operator
//! derived from the URL-target keywords. Generic single-word "terminology"
//! and "exclusion" keywords are left out — they're too noisy to seed a search.

use crate::model::{Keyword, Target};
use std::collections::HashSet;

/// Categories distinctive enough to use as phrase queries.
const PHRASE_CATEGORIES: &[&str] = &[
    "tagline",
    "crypto",
    "trading",
    "mining",
    "generic-investment",
    "marketing",
];

pub fn build_queries(keywords: &[Keyword]) -> Vec<String> {
    let phrase = |k: &Keyword| -> bool {
        !k.exclude
            && k.target == Target::Content
            && k
                .category
                .as_deref()
                .map(|c| PHRASE_CATEGORIES.contains(&c))
                .unwrap_or(false)
    };

    // inurl: operators from the URL-target keywords.
    let operators: Vec<String> = keywords
        .iter()
        .filter(|k| !k.exclude && k.target == Target::Url)
        .map(|k| format!("inurl:\"{}\"", k.phrase))
        .collect();

    let mut out: Vec<String> = Vec::new();

    // Quoted-phrase queries for every distinctive fingerprint.
    for k in keywords.iter().filter(|k| phrase(k)) {
        out.push(format!("\"{}\"", k.phrase));
    }

    // Tagline x operator combinations (taglines are the strongest signal, so
    // pairing them with a signup/register filter is a high-value dork).
    for t in keywords
        .iter()
        .filter(|k| !k.exclude && k.target == Target::Content && k.category.as_deref() == Some("tagline"))
    {
        for op in &operators {
            out.push(format!("\"{}\" {}", t.phrase, op));
        }
    }

    // Deduplicate, preserving order.
    let mut seen = HashSet::new();
    out.retain(|q| seen.insert(q.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kw(id: &str, phrase: &str, category: &str, target: Target) -> Keyword {
        Keyword {
            id: id.into(),
            phrase: phrase.into(),
            category: Some(category.into()),
            regex: false,
            whole_word: false,
            target,
            weight: None,
            exclude: false,
        }
    }

    #[test]
    fn makes_phrase_and_combo_queries() {
        let kws = vec![
            kw("t", "Dedicated to innovating", "tagline", Target::Content),
            kw("c", "Bitcoin Investment", "crypto", Target::Content),
            kw("u", "register", "signup-url", Target::Url),
        ];
        let q = build_queries(&kws);
        assert!(q.contains(&"\"Dedicated to innovating\"".to_string()));
        assert!(q.contains(&"\"Bitcoin Investment\"".to_string()));
        // tagline x operator
        assert!(q.contains(&"\"Dedicated to innovating\" inurl:\"register\"".to_string()));
        // a non-tagline phrase does NOT get combined with operators
        assert!(!q.contains(&"\"Bitcoin Investment\" inurl:\"register\"".to_string()));
    }

    #[test]
    fn skips_terminology_and_exclusions() {
        let mut term = kw("term", "invest", "terminology", Target::Content);
        term.whole_word = true;
        let mut ex = kw("ex", "bank", "exclusion", Target::Content);
        ex.exclude = true;
        ex.whole_word = true;
        let q = build_queries(&[term, ex]);
        assert!(q.is_empty());
    }
}
