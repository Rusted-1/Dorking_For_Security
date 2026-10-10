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

/// A human label for a score.
pub fn band(score: u32) -> &'static str {
    match score {
        0 => "none",
        1..=24 => "Low",
        25..=49 => "Medium",
        50..=74 => "High",
        _ => "Very High",
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
