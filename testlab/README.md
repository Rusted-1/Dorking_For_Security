# Dorker test lab (offline)

A safe place to try Dorker. Six fake websites and a fake Serper search API
run on your own PC (`localhost`, ports 8600–8606). Dorker is pointed at them by
`testlab/lab-config.toml`, so:

- no real website is contacted,
- no Serper credits are used (the "API key" is `lab-fake-key`), and
- if you have `SERPER_API_KEY` set, Dorker never sends it to the fake Serper.

Every request the fake sites receive is listed in `testlab/out/requests.log`.

| Port | Fake site | What Dorker should do |
|---|---|---|
| 8601 | Scam template (taglines, crypto phrases, `/user/register`) | Flag it, score 50+ |
| 8602 | Community bank | Exclude it as legitimate |
| 8603 | Gardening blog | No hits |
| 8604 | Club site whose `robots.txt` blocks `/private` | Never fetch `/private/offer.html` |
| 8605 | Finance chat with a few generic words | Low score (1–49) |
| 8606 | Page using two of the repaired Chinese keyword phrases | Match `ind-2062` and get flagged (65): those indicator phrases weigh 28 and 19 |
| 8607 | Nothing (a dead site) | Report an error, keep going |

The fake Serper returns all of these, plus a duplicate of 8601 and a
`skipme.invalid` link that `skip_domains` must drop (`.invalid` addresses can
never resolve, so even a bug couldn't reach the internet).

You need `dorker.exe` in the folder above `testlab` (next to `keywords.json`),
or pass `-Exe path\to\dorker.exe`. The test kit from GitHub Actions is laid
out that way already.

## 1. Automatic test

Double-click **`Run-Lab-Test.bat`**. It runs Dorker twice, starts the control
panel in the background, and checks about 30 things: credit cap, filtering,
scores, robots.txt, skipped images and PDFs, query rotation, re-scan skipping,
the panel's review and security checks, and `dorker review`. It ends with
"All N checks passed" or lists what failed. Logs are in `testlab/out/`.

## 2. Click through the control panel

Double-click **`Lab-Panel.bat`**. Your browser opens the panel at
<http://127.0.0.1:8787>, and a second window lists every request the fake
sites get. Then:

1. **Status** shows zeros. "Auto-discovery is set up ✓" is shown, because the lab key is filled in.
2. Type `2` in **Stop after N positives**, click **▶ Start**. Watch **jobs running** go to 1 and the **Activity log** fill in.
3. When it finishes, **Top hits** lists `localhost:8601` (98) and `localhost:8606` (65). Click one: its report opens in a new tab.
4. **awaiting review** shows 2. Click **✓ Confirm** on 8601: it shows "confirmed" and the counter drops to 1.
5. Click **✗ False positive** on 8606 (it is ordinary software text): it disappears and the counter drops to 0.
6. Click **▶ Start** again: the log says "Kept 0 new site(s)" because everything was already scanned.
7. Paste `http://localhost:8603/` into **Scan one site** and click **Scan it**.
8. Type `1` in **Repeat every N minutes**, click **▶ Start**, then **■ Stop**. The red STOP banner appears; **Reset everything** clears it.
9. Optional: **Setup**. Saving a key keeps the lab's fake Serper address, so it still won't reach the real API.
10. Click **Quit panel**. The lab stops too.

## 3. Compare with the current release

1. Download the current `dorker.exe` from the repo's **windows-build** release and put it in `testlab\old\dorker.exe`.
2. Double-click **`Compare-Builds.bat`**.

Both builds scan the six fake sites with the same settings. You get each
site's score from both builds, how many requests each made, and the total
time. Expect the same or slightly higher scores from the new build (phrases
split across tags and curly/straight quotes now match), and fewer requests
(robots.txt once per site, no images or PDFs).

## When the lab passes

The next step is a small real run, which does use the internet and a few
Serper credits:

```
dorker auto --max-positives 3
```
