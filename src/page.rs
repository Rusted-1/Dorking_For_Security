//! Lightweight HTML reading: a page's visible text and its links, in one pass.
//!
//! Not a full HTML parser (that pulled in a large dependency tree) — just
//! enough for keyword matching. Tags and comments are dropped, `<style>`
//! bodies are skipped, `<script>`/`<title>`/`<textarea>` bodies are kept as
//! text without parsing tags inside them, common character entities are
//! decoded, and whitespace runs collapse to one space (so a phrase split
//! across tags or lines still matches). `href`s of `<a>` tags are collected.

use std::borrow::Cow;

pub struct Page {
    /// Visible text, whitespace-collapsed.
    pub text: String,
    /// Raw `href` values of `<a>` tags (entity-decoded, not yet resolved).
    pub links: Vec<String>,
}

pub fn parse(html: &str) -> Page {
    let b = html.as_bytes();
    let mut page = Page {
        text: String::with_capacity(html.len() / 3),
        links: Vec::new(),
    };
    let mut i = 0;
    let mut run = 0; // start of the pending text run
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let next = b.get(i + 1).copied().unwrap_or(0);
        if !(next.is_ascii_alphabetic() || matches!(next, b'/' | b'!' | b'?')) {
            i += 1; // a literal '<' in text
            continue;
        }
        push_text(&mut page.text, &html[run..i]);
        if html[i..].starts_with("<!--") {
            i = find(b, i + 4, b"-->").map(|e| e + 3).unwrap_or(b.len());
        } else if next != b'/' && next != b'!' && next != b'?' {
            let tag = read_tag(html, i + 1);
            i = tag.end;
            if let Some(h) = tag.href {
                page.links.push(decode(h).trim().to_string());
            }
            let raw = matches!(tag.name.as_str(), "script" | "style" | "title" | "textarea");
            if raw && !tag.self_closing {
                let close = find_ci(b, i, format!("</{}", tag.name).as_bytes()).unwrap_or(b.len());
                match tag.name.as_str() {
                    "style" => {}
                    "script" => push_words(&mut page.text, &html[i..close]),
                    _ => push_text(&mut page.text, &html[i..close]),
                }
                i = find(b, close, b">").map(|e| e + 1).unwrap_or(b.len());
            }
        } else {
            // End tag, <!DOCTYPE …>, or <?…?>: skip it.
            i = find(b, i, b">").map(|e| e + 1).unwrap_or(b.len());
        }
        run = i;
    }
    push_text(&mut page.text, &html[run.min(b.len())..]);
    page
}

/// Decode entities in a text run and append it, whitespace-collapsed.
fn push_text(out: &mut String, raw: &str) {
    push_words(out, &decode(raw));
}

fn push_words(out: &mut String, s: &str) {
    for w in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
}

struct Tag<'a> {
    name: String,
    /// Index just past the closing '>'.
    end: usize,
    self_closing: bool,
    href: Option<&'a str>,
}

/// Read a start tag beginning at `start` (just past '<'). Every index this
/// stops at is an ASCII byte, so all slices fall on char boundaries.
fn read_tag(html: &str, start: usize) -> Tag<'_> {
    let b = html.as_bytes();
    let mut i = start;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
        i += 1;
    }
    let name = html[start..i].to_ascii_lowercase();
    let mut href = None;
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= b.len() {
            return Tag { name, end: b.len(), self_closing: false, href };
        }
        if b[i] == b'>' {
            let self_closing = b[i - 1] == b'/';
            return Tag { name, end: i + 1, self_closing, href };
        }
        let name_start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        let attr = &html[name_start..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let (vs, ve);
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                i += 1;
                vs = i;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                ve = i;
                if i < b.len() {
                    i += 1;
                }
            } else {
                vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
                ve = i;
            }
            if name == "a" && href.is_none() && attr.eq_ignore_ascii_case("href") {
                href = Some(&html[vs..ve]);
            }
        }
        if i == name_start {
            i += 1; // always make progress on odd input
        }
    }
}

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

fn find_ci(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
        .map(|p| p + from)
}

/// Decode character references (numeric, plus the common named ones).
/// Unknown references are left as written.
pub fn decode(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let after = &rest[p + 1..];
        let semi = after.bytes().take(12).position(|c| c == b';');
        if let Some(ch) = semi.and_then(|n| entity(&after[..n])) {
            out.push(ch);
            rest = &after[semi.unwrap_or(0) + 1..];
        } else {
            out.push('&');
            rest = after;
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let n = match num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(n);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "bull" => '•',
        "middot" => '·',
        "times" => '×',
        "deg" => '°',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_visible_text_and_links() {
        let html = r#"<!doctype html><html><head><title>Best Invest</title>
            <style>.x{color:red}</style></head>
            <body><!-- hidden <b>note</b> -->
            <p>Investment
               <b>Platform</b> &amp; more&nbsp;here</p>
            <a class="btn" href='/register?a=1&amp;b=2'>Sign up</a>
            <A HREF=https://other.example/x>x</A>
            <script>var s = "<div>Bitcoin Investment</div>";</script>
            <img src="a.png"/> 1 < 2
            </body></html>"#;
        let p = parse(html);
        assert_eq!(
            p.text,
            "Best Invest Investment Platform & more here Sign up x \
             var s = \"<div>Bitcoin Investment</div>\"; 1 < 2"
        );
        assert_eq!(p.links, vec!["/register?a=1&b=2", "https://other.example/x"]);
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(decode("We&rsquo;re &#36;5 &#x41; &bogus; a&b"), "We’re $5 A &bogus; a&b");
        assert!(matches!(decode("plain"), Cow::Borrowed(_)));
    }

    #[test]
    fn survives_truncated_markup() {
        // None of these may panic or loop.
        for s in ["<", "<a", "<a href=\"x", "<!--", "<script>abc", "<p class=>t", "é<b>ü</b>"] {
            let _ = parse(s);
        }
        assert_eq!(parse("é<b>ü</b>").text, "é ü");
    }
}
