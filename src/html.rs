//! Render a self-contained HTML report.

use crate::model::{Report, SiteResult};
use crate::score;

/// Escape text for safe insertion into HTML.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const STYLE: &str = r#"
:root { --bg:#f6f7f9; --card:#fff; --ink:#1a1d21; --muted:#6b7280; --line:#e4e7eb; }
@media (prefers-color-scheme: dark) {
  :root { --bg:#14171a; --card:#1d2125; --ink:#e8eaed; --muted:#9aa2ad; --line:#2b3138; }
}
* { box-sizing: border-box; }
body { margin:0; background:var(--bg); color:var(--ink);
  font:15px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif; }
.wrap { max-width: 980px; margin: 0 auto; padding: 24px 16px 64px; }
h1 { font-size: 22px; margin: 0 0 4px; }
.meta { color: var(--muted); font-size: 13px; margin-bottom: 18px; }
.stats { display:flex; gap:10px; flex-wrap:wrap; margin-bottom:18px; }
.stat { background:var(--card); border:1px solid var(--line); border-radius:10px; padding:10px 14px; }
.stat b { font-size:20px; display:block; }
.stat span { color:var(--muted); font-size:12px; }
#q { width:100%; padding:10px 12px; border:1px solid var(--line); border-radius:10px;
  background:var(--card); color:var(--ink); margin-bottom:16px; font-size:14px; }
.site { background:var(--card); border:1px solid var(--line); border-radius:12px;
  padding:12px 14px; margin-bottom:10px; }
.row { display:flex; align-items:center; gap:12px; }
.badge { flex:none; min-width:92px; text-align:center; font-weight:700; font-size:13px;
  padding:6px 8px; border-radius:8px; }
.score { font-size:16px; }
.url { word-break:break-all; }
.url a { color:inherit; text-decoration:none; }
.url a:hover { text-decoration:underline; }
.count { color:var(--muted); font-size:13px; flex:none; margin-left:auto; }
details { margin-top:10px; }
summary { cursor:pointer; color:var(--muted); font-size:13px; }
.hit { border-top:1px solid var(--line); padding:8px 0; }
.hit .k { font-weight:600; }
.hit .tag { color:var(--muted); font-size:12px; }
.snip { color:var(--muted); font-size:13px; margin-top:3px; font-family:ui-monospace,Menlo,Consolas,monospace; }
.section { margin:26px 0 10px; font-size:13px; text-transform:uppercase; letter-spacing:.06em; color:var(--muted); }
.excluded .badge { background:#9aa2ad; color:#fff; }
.excluded { opacity:.85; }
.band-none { background:#9aa2ad; color:#fff; }
.band-vu { background:#e5484d; color:#fff; }
.band-u  { background:#f76b15; color:#fff; }
.band-p  { background:#f5c518; color:#1a1a1a; }
.band-l  { background:#86d35b; color:#10280a; }
.band-vl { background:#2a7e2e; color:#fff; }
.band-ac { background:#17c964; color:#06250f; animation: blink 1.2s infinite; }
@keyframes blink { 0%,100% { opacity:1; } 50% { opacity:.4; } }
@media (prefers-reduced-motion: reduce) { .band-ac { animation: none; } }
.empty { color:var(--muted); }
"#;

const SCRIPT: &str = r#"
const q = document.getElementById('q');
if (q) q.addEventListener('input', () => {
  const t = q.value.toLowerCase();
  let shown = 0;
  document.querySelectorAll('.site').forEach(el => {
    const hay = el.getAttribute('data-hay') || '';
    const match = hay.indexOf(t) !== -1;
    el.style.display = match ? '' : 'none';
    if (match) shown++;
  });
  const c = document.getElementById('shown');
  if (c) c.textContent = shown;
});
"#;

pub fn render_html(report: &Report) -> String {
    let hits: Vec<&SiteResult> = report
        .results
        .iter()
        .filter(|r| !r.excluded && !r.hits.is_empty())
        .collect();
    let excluded: Vec<&SiteResult> = report.results.iter().filter(|r| r.excluded).collect();

    let mut body = String::new();

    // Search box (filters by URL or keyword text).
    body.push_str(&format!(
        "<input id=\"q\" type=\"search\" placeholder=\"Filter {} site(s) by URL or keyword…\" />\n",
        hits.len()
    ));

    if hits.is_empty() {
        body.push_str("<p class=\"empty\">No (non-excluded) sites with hits.</p>\n");
    }
    for r in &hits {
        body.push_str(&site_card(r, false));
    }

    if !excluded.is_empty() {
        body.push_str(&format!(
            "<div class=\"section\">Excluded — treated as legitimate ({})</div>\n",
            excluded.len()
        ));
        for r in &excluded {
            body.push_str(&site_card(r, true));
        }
    }

    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Dorker report</title>\n<style>{style}</style></head>\n\
         <body><div class=\"wrap\">\n\
         <h1>Dorker report</h1>\n\
         <div class=\"meta\">generated {gen}</div>\n\
         <div class=\"stats\">\
         <div class=\"stat\"><b>{total}</b><span>candidates</span></div>\
         <div class=\"stat\"><b id=\"shown\">{hitn}</b><span>with hits</span></div>\
         <div class=\"stat\"><b>{exn}</b><span>excluded</span></div>\
         </div>\n{body}\n</div>\n<script>{script}</script></body></html>\n",
        style = STYLE,
        gen = esc(&report.generated_at),
        total = report.total_candidates,
        hitn = hits.len(),
        exn = excluded.len(),
        body = body,
        script = SCRIPT,
    )
}

fn site_card(r: &SiteResult, excluded: bool) -> String {
    let class = if excluded {
        "band-none".to_string()
    } else {
        score::band_class(r.score).to_string()
    };
    let label = if excluded { "Excluded" } else { score::band(r.score) };
    let badge = if excluded {
        "—".to_string()
    } else {
        format!("{}/100", r.score)
    };

    // Haystack for client-side filtering.
    let mut hay = r.url.to_lowercase();
    for h in &r.hits {
        hay.push(' ');
        hay.push_str(&h.keyword_id.to_lowercase());
    }
    for e in &r.exclusions {
        hay.push(' ');
        hay.push_str(&e.to_lowercase());
    }

    let mut card = format!(
        "<div class=\"site{excl_cls}\" data-hay=\"{hay}\">\n\
         <div class=\"row\">\
         <div class=\"badge {class}\"><div class=\"score\">{badge}</div>{label}</div>\
         <div class=\"url\"><a href=\"{href}\" target=\"_blank\" rel=\"noopener noreferrer\">{href}</a></div>\
         <div class=\"count\">{hitn} hit(s)</div>\
         </div>\n",
        excl_cls = if excluded { " excluded" } else { "" },
        hay = esc(&hay),
        class = class,
        badge = esc(&badge),
        label = esc(label),
        href = esc(&r.url),
        hitn = r.hits.len(),
    );

    if excluded && !r.exclusions.is_empty() {
        card.push_str(&format!(
            "<div class=\"snip\">matched: {}</div>\n",
            esc(&r.exclusions.join(", "))
        ));
    }

    if !r.hits.is_empty() {
        card.push_str(&format!(
            "<details><summary>{} keyword hit(s)</summary>\n",
            r.hits.len()
        ));
        for h in &r.hits {
            let cat = h.category.as_deref().unwrap_or("-");
            card.push_str(&format!(
                "<div class=\"hit\"><span class=\"k\">{k}</span> \
                 <span class=\"tag\">({cat}, in:{inn}, +{w})</span>\
                 <div class=\"snip\">…{snip}…</div></div>\n",
                k = esc(&h.keyword_id),
                cat = esc(cat),
                inn = esc(h.matched_in),
                w = h.weight,
                snip = esc(&h.snippet),
            ));
        }
        card.push_str("</details>\n");
    }

    card.push_str("</div>\n");
    card
}
