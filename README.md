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

**Crawl** one site — give a start URL and it scans that page plus every
same-site link found on it (no candidate list needed):

```
cargo run --release -- scan --url https://example.com/
```

It scores the start page and each link on it, then stops (it does not follow
links deeper than that). Links are followed only on the start URL's own host
(a leading `www.` is ignored). `--max` is a safety cap on total pages fetched
(default 100). No config file is required for this mode.

Crawl mode also writes `out/links.md` — an inventory of every link found
(internal pages, plus external links grouped by domain) for planning future
dorks.

**Scan** a batch of candidates from a config file:

```
cargo run --release -- scan --config config.toml
```

Writes three reports into `out/` (sites ranked most-certain first):

- `report.html` — a self-contained page (open in a browser): colored score
  badges matching the bands below, a filter box, expandable per-site hits, and
  an "Excluded" section.
- `report.json` — machine-readable.
- `report.txt` — plain text (number + label).

**Check** a single page or local file against the keyword list (handy for
testing your list offline):

```
cargo run --release -- check --file some_page.html
cargo run --release -- check --url https://example.com/page
```

## Running multiple instances + stopping

Several `dorker` instances can run at once without ever touching the same
site. They share a **coordination directory**; each instance claims a whole
**site** (its host) before scanning it, and any other instance that sees the
site claimed stays off it entirely (per-URL claims add a second guard).
Coordination is on by default (a folder in the system temp dir). Point
instances at a shared path to coordinate explicitly — including across
machines if the path is on a shared filesystem:

```
dorker --coordinate /shared/dorker-run scan --url https://site-a.com/
dorker --coordinate /shared/dorker-run scan --url https://site-b.com/
```

An instance pointed at a site another instance already owns prints a notice
and exits without scanning. Use `--solo` to run with no coordination.

See and reset what's claimed:

```
dorker --coordinate /shared/dorker-run sites   # list claimed sites
dorker --coordinate /shared/dorker-run reset    # clear all claims + stop
```

Claims self-heal after 6 hours (so a crashed instance never blocks a site
forever); `reset` clears them immediately for a fresh run.

**Stop command.** To halt every running instance immediately, from any
terminal run:

```
dorker stop                                  # default coordination dir
dorker --coordinate /shared/dorker-run stop  # a specific one
```

Each running scan checks for the stop signal before every page, halts at once,
and still writes a report of the pages done so far. (Ctrl+C remains the
instant local kill.) A new scan clears any leftover stop signal automatically.

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
- `exclude` — if true, a match disqualifies the page: it is treated as
  legitimate, scored 0, and dropped from the ranked results (listed instead
  under "Excluded" for auditing). Checked against both content and URL. Use
  this for terms that signal a different, legitimate vertical (e.g. banking,
  shipping/logistics).

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

Scores map to a 6-level label and a terminal color:

| Score | Color | Label |
|---|---|---|
| 1–25 | red | Very unlikely |
| 26–50 | orange | Unlikely |
| 51–75 | yellow | Possible |
| 76–85 | light green | Likely |
| 86–90 | dark green | Very likely |
| 91–100 | flashing green | Almost certain |

Color is applied to terminal output. Control it with `--color auto|always|never`
(default `auto`: colored only when writing to a terminal and `NO_COLOR` is
unset). The saved `report.txt` always uses plain text (number + label).

Weights, the curve constant, and the bands live in `src/score.rs` and are easy
to tune.
