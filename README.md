# Dorking_For_Security

A small Rust tool for anti-fraud work: it takes a list of candidate web pages,
checks each one for scam-site "fingerprint" keywords, scores how likely the page
is a scam template (1–100), and writes an organized report.

It does **not** attack, log into, or submit anything to any site — it fetches
pages the way a browser would (rate-limited, with a timeout, honoring
`robots.txt`) and looks for text and URL patterns.

## Build

```
cargo build --release
```

## Usage

**Scan** a batch of candidates from a config file:

```
cargo run --release -- scan --config config.toml
```

Writes `out/report.json` and `out/report.txt` (sites ranked most-certain first).

**Check** a single page or local file against the keyword list (handy for
testing your list offline):

```
cargo run --release -- check --file some_page.html
cargo run --release -- check --url https://example.com/page
```

## Configuration (`config.toml`)

- `keywords_file` — path to the keyword data file.
- `output_dir` — where reports are written.
- `[fetch]` — `delay_ms` (politeness delay between requests), `timeout_secs`,
  `user_agent`, `respect_robots`.
- `[source]` — where candidate URLs come from:
  - `kind = "url_list"` with `path = "candidates.txt"` (one URL per line) —
    works with no API key; ideal for a controlled environment.
  - `kind = "search_api"`, `provider = "google_cse"` with `api_key`, `cx`,
    `max_results`, and `queries` — discovers candidates via the Google
    Programmable Search (Custom Search JSON) API.

## Keywords (`keywords.json`)

Each keyword is an object:

```json
{ "id": "the-financial-ecosystem", "phrase": "The Financial Ecosystem",
  "category": "tagline", "whole_word": false, "regex": false,
  "target": "content", "weight": null }
```

- `phrase` — matched case-insensitively.
- `category` — groups hits and sets the default scoring weight.
- `whole_word` — match on word boundaries (use for short, generic terms so
  "coin" doesn't match inside "bitcoin").
- `regex` — treat `phrase` as a regular expression.
- `target` — `"content"` (page text) or `"url"` (an `inurl:`-style filter).
- `weight` — optional override of the category's default weight.

## Scoring

Each matched keyword contributes a weight (defaults by category, below). A
site's weights are summed and mapped through a saturating curve into a 1–100
certainty score, so more and more-distinctive matches raise certainty with
diminishing returns.

| Category | Default weight |
|---|---|
| `tagline` | 40 |
| `crypto` | 12 |
| `signup-url` | 10 |
| `trading`, `mining` | 8 |
| `generic-investment`, `marketing` | 4 |
| `trading-ambiguous` | 3 |
| `terminology` | 2 |
| (anything else) | 3 |

Bands: **Low** 1–24, **Medium** 25–49, **High** 50–74, **Very High** 75–100.

Weights and the curve constant live in `src/score.rs` and are easy to tune.
