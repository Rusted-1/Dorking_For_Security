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

## Quick start (easiest path)

New to this? Two commands:

```
cargo run --release -- setup     # answer a few questions; writes config.toml
cargo run --release -- auto       # discover scam sites and scan them all
```

Prefer buttons to typing? Start the **control panel** and use it in your browser:

```
cargo run --release -- serve      # then open http://localhost:8787
```

The panel shows live status (queue, positives, claimed sites, top hits) and
has buttons for everything, so a user never touches the terminal: a big
**Start / Stop** pair, Discover, Start a worker, Reset, **Quit panel**, a
**"scan one site"** box, an add-to-queue box, and a "stop after N positives"
field. Top hits are clickable — each opens that site's full colored report.
On WSL, open it in your Windows browser with
`explorer.exe http://localhost:8787`, or double-click the included
`Dorker-Panel.bat`.

`setup` walks you through it in plain questions (including pasting your Google
key, if you want auto-discovery) and writes the config for you — no hand-editing
files. `auto` then runs the whole pipeline in one go: it discovers candidate
sites, scans each one, and writes reports to `out/sites/<host>/`. Open any
`report.html` in a browser. To check a single site without any setup:

```
cargo run --release -- scan --url https://suspect-site.com/
```

The rest of this file covers the individual commands and finer controls.

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

**Discover** candidate sites automatically with Google search dorks (needs a
`search_api` source configured — see below):

```
cargo run --release -- discover --config config.toml          # find sites -> out/discovered.txt
cargo run --release -- discover --config config.toml --scan    # find and scan them
```

With `queries = []` in the config, dorker builds the dork queries itself from
`keywords.json` (quoted phrases for the distinctive categories, plus each
tagline paired with the `inurl:` filters). You can still hand-write `queries`
to override. `discover` writes the found URLs to `out/discovered.txt`; add
`--scan` to score them in the same run. Setting up the Google key + search
engine id is described under **Auto-discovery setup** below.

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

**Work queue (self-serve workers).** Instead of launching one instance per
site, drop sites into a shared queue and run workers that pull from it on
their own. The queue lives in the coordination directory, so all the
site-claim and stop guarantees above apply.

```
dorker --coordinate /shared/run enqueue https://a.com/ https://b.com/   # add sites
dorker --coordinate /shared/run enqueue --file sites.txt                # bulk add
dorker --coordinate /shared/run queue                                   # show the queue
dorker --coordinate /shared/run work --config config.toml               # a worker drains it
dorker --coordinate /shared/run work --config config.toml --watch       # stay alive for new items
```

Run as many `work` instances as you like — each atomically takes the next
site, so no two process the same one. Each worker writes a report per site to
`out/sites/<host>/`. `discover --enqueue` feeds discovered sites straight into
the queue, so discovery → scan runs hands-off: one `discover --enqueue`, a few
`work --watch` instances, and new finds get scored as they arrive.

**Positive limit (collect a set batch, then stop).** So a run collects a
reviewable number of hits instead of running indefinitely, pass
`--max-positives` to `work` or `auto`. Once that many positive sites have been
found, the run stops and signals every instance to halt — leaving the rest of
the queue for a later batch:

```
dorker --coordinate /shared/run work --config config.toml --max-positives 10
dorker auto --config config.toml --max-positives 20
```

A site counts as a "positive" when its top score is at least
`--positive-threshold` (default 50 = "Possible" and up). The count is shared
across all instances through the coordination folder, so a limit of 10 means
10 total across the fleet, not 10 per worker. The flagged sites are also
catalogued in the coordination folder's `positives/` list.

**Status at a glance.** One command shows the queue, how many positives have
been flagged, which sites are claimed, and the current top-scoring hits:

```
dorker --coordinate /shared/run status
```

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
    Programmable Search (Custom Search JSON) API. Leave `queries = []` to
    auto-generate dorks from the keywords.

## Auto-discovery setup (Google Programmable Search)

To use `discover`, create a search engine and an API key (both free tier):

1. **Programmable Search Engine** — at <https://programmablesearchengine.google.com/>
   create an engine set to **search the entire web**. Copy its **Search engine
   ID** → that's `cx`.
2. **API key** — at <https://console.cloud.google.com/>, enable the **Custom
   Search API** for a project and create an API key → that's `api_key`.
3. Put both in `config.toml` under the `search_api` source, set
   `queries = []`, and run `dorker discover --config config.toml`.

Note the free tier allows ~100 search queries/day; each dork query uses one
(and more as it pages through results), so keep `max_results` modest while
testing. Nothing about discovery requires Claude — it's the tool calling
Google's API directly.

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
