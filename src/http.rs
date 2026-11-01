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

/// Most bytes read from one page. Scam landing pages are small; anything
/// bigger is truncated rather than downloaded in full.
const MAX_BODY_BYTES: usize = 3 * 1024 * 1024;

/// File extensions that are never web pages, so links to them aren't fetched.
const NON_PAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "svg", "ico", "bmp", "avif", "pdf", "zip", "rar", "7z",
    "gz", "tar", "exe", "msi", "apk", "dmg", "mp3", "mp4", "m4a", "wav", "avi", "mov", "webm",
    "woff", "woff2", "ttf", "otf", "eot", "css", "js", "json", "xml", "doc", "docx", "xls",
    "xlsx", "ppt", "pptx", "csv",
];

/// True unless the URL's path clearly names a non-page file (image, PDF, …).
pub fn looks_like_page(url: &Url) -> bool {
    let last = url.path().rsplit('/').next().unwrap_or("");
    match last.rsplit_once('.') {
        Some((_, ext)) => !NON_PAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()),
        None => true,
    }
}

/// Fetch one page: skips non-text responses and caps the body size.
/// Errors carry the HTTP status (when one arrived) and a reason.
pub async fn fetch_page(
    client: &reqwest::Client,
    url: Url,
) -> std::result::Result<(u16, String), (Option<u16>, String)> {
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| (None, format!("request failed: {e}")))?;
    let status = resp.status().as_u16();
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let texty = ctype.is_empty()
        || ctype.starts_with("text/")
        || ctype.contains("html")
        || ctype.contains("xml");
    if !texty {
        return Err((Some(status), format!("skipped: not a web page ({ctype})")));
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| (Some(status), format!("reading body: {e}")))?
    {
        let room = MAX_BODY_BYTES - buf.len();
        buf.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if buf.len() >= MAX_BODY_BYTES {
            break;
        }
    }
    Ok((status, String::from_utf8_lossy(&buf).into_owned()))
}

/// robots.txt checks with each site's file fetched once, not once per page.
///
/// Conservative and intentionally simple: if robots.txt can't be fetched (or
/// isn't served) we allow the request (fail open), which matches how most
/// crawlers behave when robots.txt is unreachable.
#[derive(Default)]
pub struct RobotsCache {
    /// origin -> robots.txt body (None = unavailable, allow everything)
    bodies: HashMap<String, Option<String>>,
}

impl RobotsCache {
    pub async fn allows(&mut self, client: &reqwest::Client, target: &Url, user_agent: &str) -> bool {
        let origin = target.origin().ascii_serialization();
        if !self.bodies.contains_key(&origin) {
            let body = fetch_robots(client, target).await;
            self.bodies.insert(origin.clone(), body);
        }
        match self.bodies.get(&origin) {
            Some(Some(body)) => {
                let ua_token = user_agent.split('/').next().unwrap_or(user_agent);
                path_allowed(body, ua_token, target.path())
            }
            _ => true,
        }
    }
}

async fn fetch_robots(client: &reqwest::Client, target: &Url) -> Option<String> {
    let robots_url = target.join("/robots.txt").ok()?;
    let resp = client.get(robots_url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.text().await.ok()
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
    use super::{looks_like_page, path_allowed};

    #[test]
    fn skips_obvious_non_pages() {
        let page = |s: &str| looks_like_page(&url::Url::parse(s).unwrap());
        assert!(page("https://a.example/"));
        assert!(page("https://a.example/register"));
        assert!(page("https://a.example/index.php?x=1"));
        assert!(page("https://a.example/v1.2/about"));
        assert!(!page("https://a.example/logo.PNG"));
        assert!(!page("https://a.example/files/terms.pdf"));
    }

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
