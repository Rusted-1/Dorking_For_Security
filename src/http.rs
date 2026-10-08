//! Shared HTTP client and a minimal robots.txt check.

use crate::config::FetchConfig;
use anyhow::Result;
use std::collections::HashMap;
use std::time::Duration;
use url::Url;

pub fn build_client(cfg: &FetchConfig) -> Result<reqwest::Client> {
    let client = reqwest::Client::builder()
        .user_agent(cfg.user_agent.clone())
        .timeout(Duration::from_secs(cfg.timeout_secs))
        .build()?;
    Ok(client)
}

/// Decide whether `url` may be fetched according to the site's robots.txt.
///
/// Conservative and intentionally simple: on any error fetching or parsing
/// robots.txt we allow the request (fail open), which matches how most
/// crawlers behave when robots.txt is unreachable.
pub async fn robots_allows(client: &reqwest::Client, target: &Url, user_agent: &str) -> bool {
    let robots_url = match target.join("/robots.txt") {
        Ok(u) => u,
        Err(_) => return true,
    };
    let body = match client.get(robots_url).send().await {
        Ok(resp) => match resp.text().await {
            Ok(b) => b,
            Err(_) => return true,
        },
        Err(_) => return true,
    };
    let ua_token = user_agent.split('/').next().unwrap_or(user_agent);
    path_allowed(&body, ua_token, target.path())
}

/// Parse robots.txt into user-agent groups and test a path.
fn path_allowed(robots: &str, ua_token: &str, path: &str) -> bool {
    // group name (lowercased) -> list of (is_allow, path_prefix)
    let mut groups: HashMap<String, Vec<(bool, String)>> = HashMap::new();
    let mut current: Vec<String> = Vec::new();
    let mut last_was_ua = false;

    for line in robots.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        let field = field.trim().to_lowercase();
        let value = value.trim().to_string();
        match field.as_str() {
            "user-agent" => {
                if !last_was_ua {
                    current.clear();
                }
                current.push(value.to_lowercase());
                last_was_ua = true;
            }
            "disallow" | "allow" => {
                last_was_ua = false;
                let is_allow = field == "allow";
                for agent in &current {
                    groups
                        .entry(agent.clone())
                        .or_default()
                        .push((is_allow, value.clone()));
                }
            }
            _ => {
                last_was_ua = false;
            }
        }
    }

    let ua_lower = ua_token.to_lowercase();
    let rules = groups
        .get(&ua_lower)
        .or_else(|| groups.get("*"))
        .cloned()
        .unwrap_or_default();

    // Longest matching rule wins; Allow beats Disallow on a tie.
    let mut best: Option<(usize, bool)> = None;
    for (is_allow, prefix) in &rules {
        if prefix.is_empty() {
            continue;
        }
        if path.starts_with(prefix) {
            let len = prefix.len();
            match best {
                Some((best_len, _)) if best_len > len => {}
                Some((best_len, _)) if best_len == len && !is_allow => {}
                _ => best = Some((len, *is_allow)),
            }
        }
    }
    match best {
        Some((_, is_allow)) => is_allow,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::path_allowed;

    #[test]
    fn disallow_blocks_prefix() {
        let robots = "User-agent: *\nDisallow: /private";
        assert!(!path_allowed(robots, "Dorker", "/private/page"));
        assert!(path_allowed(robots, "Dorker", "/public"));
    }

    #[test]
    fn empty_disallow_allows_all() {
        let robots = "User-agent: *\nDisallow:";
        assert!(path_allowed(robots, "Dorker", "/anything"));
    }

    #[test]
    fn allow_overrides_longer_disallow() {
        let robots = "User-agent: *\nDisallow: /a\nAllow: /a/b";
        assert!(path_allowed(robots, "Dorker", "/a/b/c"));
        assert!(!path_allowed(robots, "Dorker", "/a/x"));
    }
}
