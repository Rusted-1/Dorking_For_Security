//! Certainty scoring.
//!
//! Each matched keyword contributes a weight. A site's weights are summed and
//! mapped through a saturating curve into a 1-100 certainty score, so that
//! more (and more distinctive) matches raise certainty with diminishing
//! returns — a handful of generic finance words can't reach the top on their
//! own, but a distinctive tagline plus corroborating terms can.

/// Default weight for a keyword, chosen by its category. Any keyword may
/// override this with an explicit `weight` field in keywords.json.
pub fn default_weight(category: Option<&str>) -> u32 {
    match category {
        Some("tagline") => 40,
        Some("crypto") => 12,
        Some("signup-url") => 10,
        Some("trading") => 8,
        Some("mining") => 8,
        Some("generic-investment") => 4,
        Some("marketing") => 4,
        Some("trading-ambiguous") => 3,
        Some("terminology") => 2,
        _ => 3,
    }
}

/// The curve's "half-life"-ish constant. Larger = slower approach to 100.
const K: f64 = 45.0;

/// Map a summed weight to a 1-100 certainty (0 only when nothing matched).
pub fn certainty(weight_total: u32) -> u32 {
    if weight_total == 0 {
        return 0;
    }
    let s = (1.0 - (-(weight_total as f64) / K).exp()) * 100.0;
    (s.round() as u32).clamp(1, 100)
}

/// A human label for a score (6-level scale, aligned with the color bands).
pub fn band(score: u32) -> &'static str {
    match score {
        0 => "none",
        1..=25 => "Very unlikely",
        26..=50 => "Unlikely",
        51..=75 => "Possible",
        76..=85 => "Likely",
        86..=90 => "Very likely",
        _ => "Almost certain", // 91..=100
    }
}

/// ANSI color-escape prefix for a score band. Pair with [`RESET`].
pub fn ansi(score: u32) -> &'static str {
    match score {
        0 => "",
        1..=25 => "\x1b[31m",        // red
        26..=50 => "\x1b[38;5;208m", // orange
        51..=75 => "\x1b[33m",       // yellow
        76..=85 => "\x1b[38;5;120m", // light green
        86..=90 => "\x1b[38;5;28m",  // dark green
        _ => "\x1b[5;92m",           // flashing bright green (91-100)
    }
}

pub const RESET: &str = "\x1b[0m";

/// Format `score`/100 and its label, colored when `use_color` is true.
pub fn tag(score: u32, use_color: bool) -> String {
    if use_color {
        format!("{}{:>3}/100 {}{}", ansi(score), score, band(score), RESET)
    } else {
        format!("{:>3}/100 {}", score, band(score))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_and_bounded() {
        assert_eq!(certainty(0), 0);
        assert!(certainty(2) >= 1);
        assert!(certainty(10) < certainty(40));
        assert!(certainty(500) <= 100);
    }

    #[test]
    fn one_tagline_is_at_least_high_medium() {
        // A single distinctive tagline (weight 40) should land in High.
        assert!(certainty(40) >= 50);
    }

    #[test]
    fn a_few_generic_words_stay_low() {
        // Three single finance words (weight 2 each) must not look likely.
        assert!(certainty(6) < 25);
    }
}
