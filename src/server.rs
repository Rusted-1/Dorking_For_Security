//! A small local web control panel: `dorker serve`.
//!
//! Serves a page with buttons (discover, auto, worker, stop, reset, enqueue)
//! and a status view that refreshes itself. Action buttons launch the matching
//! `dorker` command as a child process (sharing the same coordination folder),
//! so the panel is just a friendly front end over the CLI. Bound to localhost.

use crate::coord::Coordinator;
use crate::state::{State, Verdict};
use crate::status;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use tiny_http::{Header, Method, Request, Response, Server};

/// Header the panel's own page sends on every POST. A cross-site form or
/// fetch can't add a custom header without a CORS preflight (which this
/// server never approves), so requiring it blocks other web pages from
/// driving the panel.
const PANEL_HEADER: &str = "X-Dorker-Panel";

/// Lines of the activity log shown in the panel.
const LOG_TAIL_LINES: usize = 60;

pub fn run_serve(
    port: u16,
    config_path: &Path,
    output_dir: &str,
    coord: &Coordinator,
) -> Result<()> {
    let addr = format!("127.0.0.1:{port}");
    let server = Server::http(&addr).map_err(|e| anyhow!("starting web panel: {e}"))?;
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("dorker"));
    let coord_dir = coord.path().to_path_buf();
    let config = config_path.to_string_lossy().to_string();
    let log_path = Path::new(output_dir).join("dorker.log");
    let mut children: Vec<Child> = Vec::new();

    println!("Dorker control panel: http://{addr}");
    println!("(Leave this running. Open that address in your browser. Ctrl+C to stop the panel.)");
    println!("Background jobs log to {}", log_path.display());

    for req in server.incoming_requests() {
        let method = req.method().clone();
        let raw = req.url().to_string();
        let path = raw.split('?').next().unwrap_or("/").to_string();

        if let Err(why) = check_request(&req, port) {
            let _ = req.respond(Response::from_string(why).with_status_code(403));
            continue;
        }
        let mut spawn = |args: Vec<String>| {
            if let Some(c) = spawn_child(&exe, &coord_dir, &args, &log_path) {
                children.push(c);
            }
        };

        match (&method, path.as_str()) {
            (Method::Get, "/") => {
                respond_html(req, PAGE);
            }
            (Method::Get, "/api/status") => {
                children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
                respond_json(req, &status_json(coord, output_dir, &config, children.len()));
            }
            (Method::Get, "/api/log") => {
                let text = tail(&log_path, LOG_TAIL_LINES);
                let header =
                    Header::from_bytes(&b"Content-Type"[..], &b"text/plain; charset=utf-8"[..]).unwrap();
                let _ = req.respond(Response::from_string(text).with_header(header));
            }
            (Method::Post, "/api/review") => {
                let mut body = String::new();
                let mut req = req;
                let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
                let v: serde_json::Value =
                    serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!({}));
                let host = v.get("host").and_then(|x| x.as_str()).unwrap_or("").trim();
                let verdict = v.get("verdict").and_then(|x| x.as_str()).unwrap_or("");
                let parsed = if verdict == "clear" { None } else { Verdict::parse(verdict) };
                if host.is_empty() || (parsed.is_none() && verdict != "clear") {
                    respond_json(req, r#"{"ok":false,"msg":"Missing site or verdict."}"#);
                } else {
                    match State::open(Path::new(output_dir)).set_review(host, parsed) {
                        Ok(_) => respond_json(req, r#"{"ok":true,"msg":"Review saved."}"#),
                        Err(_) => respond_json(req, r#"{"ok":false,"msg":"Could not save the review."}"#),
                    }
                }
            }
            (Method::Post, "/api/setup") => {
                let mut body = String::new();
                let mut req = req;
                let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
                let v: serde_json::Value =
                    serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!({}));
                let api_key = v.get("api_key").and_then(|x| x.as_str()).unwrap_or("").trim();
                let delay = v.get("delay").and_then(|x| x.as_u64()).unwrap_or(1500);
                let outd = v.get("output_dir").and_then(|x| x.as_str()).unwrap_or("out");
                match crate::config::write_config_file(Path::new(&config), "serper", api_key, "", delay, outd) {
                    Ok(_) => respond_json(req, r#"{"ok":true,"msg":"Settings saved."}"#),
                    Err(e) => respond_json(
                        req,
                        &serde_json::json!({"ok": false, "msg": format!("Could not save: {e}")}).to_string(),
                    ),
                }
            }
            (Method::Post, "/api/discover") => {
                spawn(strings(&["discover", "--config", &config, "--enqueue"]));
                respond_json(req, r#"{"ok":true,"msg":"Discovery started."}"#);
            }
            (Method::Post, "/api/auto") => {
                let mut args = vec!["auto".to_string(), "--config".into(), config.clone()];
                if let Some(n) = query_num(&raw, "positives") {
                    args.push("--max-positives".into());
                    args.push(n.to_string());
                }
                let every = query_num(&raw, "every");
                if let Some(m) = every {
                    args.push("--every".into());
                    args.push(m.to_string());
                }
                spawn(args);
                let msg = match every {
                    Some(m) => format!("Continuous run started (a new round every {m} min)."),
                    None => "Auto run started.".to_string(),
                };
                respond_json(req, &serde_json::json!({"ok": true, "msg": msg}).to_string());
            }
            (Method::Post, "/api/worker") => {
                let mut args = vec![
                    "work".to_string(),
                    "--config".into(),
                    config.clone(),
                    "--watch".into(),
                ];
                if let Some(n) = query_num(&raw, "positives") {
                    args.push("--max-positives".into());
                    args.push(n.to_string());
                }
                spawn(args);
                respond_json(req, r#"{"ok":true,"msg":"Worker started (watching the queue)."}"#);
            }
            (Method::Post, "/api/scan-url") => {
                let mut body = String::new();
                let mut req = req;
                let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
                let url = body.trim().lines().next().unwrap_or("").trim().to_string();
                if url.is_empty() {
                    respond_json(req, r#"{"ok":false,"msg":"Enter a URL first."}"#);
                } else {
                    coord.enqueue(&url);
                    spawn(strings(&["work", "--config", &config]));
                    respond_json(req, r#"{"ok":true,"msg":"Scanning that site…"}"#);
                }
            }
            (Method::Post, "/api/stop") => {
                let _ = coord.request_stop();
                respond_json(req, r#"{"ok":true,"msg":"Stop signal sent."}"#);
            }
            (Method::Post, "/api/quit") => {
                respond_json(req, r#"{"ok":true,"msg":"Shutting the panel down…"}"#);
                println!("Panel shut down from the browser. Goodbye.");
                std::process::exit(0);
            }
            (Method::Get, p) if p.starts_with("/reports/") => {
                serve_report_file(req, output_dir, &p["/reports/".len()..]);
            }
            (Method::Post, "/api/reset") => {
                let _ = coord.reset();
                respond_json(req, r#"{"ok":true,"msg":"Coordination state cleared."}"#);
            }
            (Method::Post, "/api/enqueue") => {
                let mut body = String::new();
                let mut req = req;
                let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
                let mut added = 0usize;
                for line in body.lines() {
                    let u = line.trim();
                    if u.is_empty() || u.starts_with('#') {
                        continue;
                    }
                    if coord.enqueue(u) {
                        added += 1;
                    }
                }
                respond_json(req, &format!(r#"{{"ok":true,"msg":"Queued {added} URL(s)."}}"#));
            }
            _ => {
                let _ = req.respond(Response::from_string("not found").with_status_code(404));
            }
        }
    }
    Ok(())
}

/// Reject requests that didn't come from the panel's own page: the Host must
/// be this loopback address (defeats DNS rebinding), and every POST must carry
/// the panel header and, if the browser sent one, a matching Origin.
fn check_request(req: &Request, port: u16) -> std::result::Result<(), &'static str> {
    let header = |name: &str| {
        req.headers()
            .iter()
            .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str().to_string())
    };
    let allowed = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    match header("Host") {
        Some(h) if allowed.iter().any(|a| a.eq_ignore_ascii_case(&h)) => {}
        _ => return Err("forbidden host"),
    }
    if *req.method() == Method::Post {
        if header(PANEL_HEADER).is_none() {
            return Err("missing panel header");
        }
        if let Some(origin) = header("Origin") {
            if !allowed.iter().any(|a| origin.eq_ignore_ascii_case(&format!("http://{a}"))) {
                return Err("forbidden origin");
            }
        }
    }
    Ok(())
}

fn status_json(coord: &Coordinator, output_dir: &str, config_path: &str, jobs: usize) -> String {
    let (pending, processing) = coord.queue_counts();
    let positives = coord.positive_count();
    let sites = coord.list_sites().len();
    let stop = coord.stop_requested();
    // Discovery is configured when a search_api source has a key (file or env).
    let configured = crate::config::load_config(Path::new(config_path))
        .map(|c| c.source.discovery_ready())
        .unwrap_or(false);
    // One read of the reports serves both the counter and the list.
    let hits = status::top_hits(Path::new(output_dir), usize::MAX);
    let awaiting = hits.iter().filter(|h| h.score >= 50 && h.review.is_none()).count();
    let hits_json: Vec<serde_json::Value> = hits
        .iter()
        .take(25)
        .map(|h| {
            let report = format!("/reports/sites/{}/report.html", crate::coord::safe_host(&h.url));
            serde_json::json!({
                "score": h.score,
                "url": h.url,
                "host": h.host,
                "review": h.review.map(|v| v.as_str()),
                "report": report,
            })
        })
        .collect();
    serde_json::json!({
        "pending": pending,
        "processing": processing,
        "positives": positives,
        "sites": sites,
        "stop": stop,
        "configured": configured,
        "jobs": jobs,
        "awaiting": awaiting,
        "hits": hits_json,
    })
    .to_string()
}

/// The last `n` lines of a text file ("" if it doesn't exist yet).
fn tail(path: &Path, n: usize) -> String {
    let txt = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = txt.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

fn query_num(raw: &str, key: &str) -> Option<u64> {
    let q = raw.split_once('?')?.1;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return v.parse().ok();
        }
    }
    None
}

/// Launch a `dorker` subcommand in the background, appending its output to
/// the activity log so the panel can show what it's doing.
fn spawn_child(exe: &Path, coord_dir: &Path, args: &[String], log_path: &Path) -> Option<Child> {
    let mut cmd = Command::new(exe);
    cmd.arg("--color").arg("never");
    cmd.arg("--coordinate").arg(coord_dir);
    cmd.args(args);
    if let Some(dir) = log_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
        use std::io::Write;
        let _ = writeln!(
            log,
            "\n[{}] $ dorker {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            args.join(" ")
        );
        if let Ok(err) = log.try_clone() {
            cmd.stdout(Stdio::from(log)).stderr(Stdio::from(err));
        }
    }
    cmd.spawn().ok()
}

/// Serve a saved report file under `output_dir` (read-only, no path escapes).
fn serve_report_file(req: tiny_http::Request, output_dir: &str, rel: &str) {
    // Reject anything that could climb out of the output directory.
    if rel.contains("..") || rel.starts_with('/') {
        let _ = req.respond(Response::from_string("bad path").with_status_code(400));
        return;
    }
    let full = Path::new(output_dir).join(rel);
    match std::fs::read(&full) {
        Ok(bytes) => {
            let ctype: &[u8] = if rel.ends_with(".html") {
                b"text/html; charset=utf-8"
            } else if rel.ends_with(".json") {
                b"application/json"
            } else {
                b"text/plain; charset=utf-8"
            };
            let header = Header::from_bytes(&b"Content-Type"[..], ctype).unwrap();
            let _ = req.respond(Response::from_data(bytes).with_header(header));
        }
        Err(_) => {
            let _ = req.respond(Response::from_string("report not found").with_status_code(404));
        }
    }
}

fn respond_html(req: tiny_http::Request, body: &str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap();
    let _ = req.respond(Response::from_string(body).with_header(header));
}

fn respond_json(req: tiny_http::Request, body: &str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let _ = req.respond(Response::from_string(body).with_header(header));
}

const PAGE: &str = r##"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Dorker Control Panel</title>
<style>
  :root{--bg:#f4f6f8;--surface:#fff;--fg:#14181d;--muted:#5b6471;--line:#e2e6ea;--accent:#0d9488;--accent-ink:#fff;
    --vu:#e5484d;--u:#f76b15;--p:#f5c518;--l:#86d35b;--vl:#2a7e2e;--ac:#17c964;
    --sans:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;
    --mono:ui-monospace,Menlo,Consolas,monospace}
  @media (prefers-color-scheme:dark){:root{--bg:#0f1216;--surface:#171b21;--fg:#e7eaee;--muted:#98a2b0;--line:#262c34;--accent:#2dd4bf;--accent-ink:#06231f;color-scheme:dark}}
  *{box-sizing:border-box} body{margin:0;background:var(--bg);color:var(--fg);font-family:var(--sans);line-height:1.5}
  .wrap{max-width:820px;margin:0 auto;padding:28px 16px 64px}
  h1{font-size:22px;margin:0 0 2px} .sub{color:var(--muted);font-size:13px;margin:0 0 22px}
  .card{background:var(--surface);border:1px solid var(--line);border-radius:12px;padding:16px;margin-bottom:16px}
  .card h2{font-size:12px;font-family:var(--mono);text-transform:uppercase;letter-spacing:.1em;color:var(--muted);margin:0 0 12px}
  .stats{display:flex;flex-wrap:wrap;gap:10px}
  .stat{flex:1;min-width:110px;background:var(--bg);border:1px solid var(--line);border-radius:10px;padding:10px 12px}
  .stat b{font-size:22px;display:block;font-variant-numeric:tabular-nums} .stat span{color:var(--muted);font-size:12px}
  .row{display:flex;flex-wrap:wrap;gap:10px;align-items:center}
  button{font:inherit;font-weight:600;border:1px solid var(--line);background:var(--surface);color:var(--fg);
    padding:9px 14px;border-radius:9px;cursor:pointer}
  button:hover{border-color:var(--accent)}
  button.primary{background:var(--accent);color:var(--accent-ink);border-color:var(--accent)}
  button.danger{color:#fff;background:#e5484d;border-color:#e5484d}
  button.big{font-size:16px;padding:12px 20px}
  label{font-size:13px;color:var(--muted)} input[type=number]{width:72px}
  input,textarea{font:inherit;background:var(--bg);color:var(--fg);border:1px solid var(--line);border-radius:8px;padding:8px}
  textarea{width:100%;min-height:68px;font-family:var(--mono);font-size:12px}
  .hit{display:flex;align-items:center;gap:10px;padding:6px 0;border-top:1px solid var(--line);word-break:break-all}
  .hit:first-child{border-top:0}
  .hit .lbl{flex:none;font-size:13px;color:var(--muted)} .hit .u{flex:1;min-width:0}
  .hit .acts{flex:none;display:flex;gap:6px;align-items:center}
  button.small{padding:4px 9px;font-size:12px;border-radius:7px}
  .tag{font-size:12px;font-weight:700;color:var(--accent)}
  .log{margin:0;max-height:260px;overflow:auto;background:var(--bg);border:1px solid var(--line);border-radius:8px;
    padding:10px;font-family:var(--mono);font-size:12px;white-space:pre-wrap;word-break:break-all}
  @media (max-width:560px){.hit{flex-wrap:wrap}.hit .acts{width:100%}}
  .badge{flex:none;min-width:78px;text-align:center;font-weight:700;font-size:12px;font-family:var(--mono);padding:4px 6px;border-radius:7px}
  .b-vu{background:var(--vu);color:#fff}.b-u{background:var(--u);color:#fff}.b-p{background:var(--p);color:#1a1a1a}
  .b-l{background:var(--l);color:#10280a}.b-vl{background:var(--vl);color:#fff}
  .b-ac{background:var(--ac);color:#06250f;animation:blink 1.2s infinite}
  @keyframes blink{0%,100%{opacity:1}50%{opacity:.4}}
  @media (prefers-reduced-motion:reduce){.b-ac{animation:none}}
  .muted{color:var(--muted);font-size:13px} #msg{min-height:18px;color:var(--accent);font-size:13px;font-weight:600}
  .stopbanner{background:#e5484d;color:#fff;border-radius:9px;padding:8px 12px;font-size:13px;font-weight:600;margin-bottom:14px}
</style></head><body><div class="wrap">
  <h1>Dorker Control Panel</h1>
  <p class="sub">Scam-site scanner — buttons for the common actions. Keep the terminal that launched this open.</p>

  <div id="stopbanner" class="stopbanner" hidden>A STOP signal is set — runs will halt. Click Reset to clear it.</div>

  <div class="card">
    <h2>Setup (one time)</h2>
    <p class="muted" style="margin-top:0">For auto-discovery, paste your free Serper API key (get one at serper.dev), then Save. Leave blank to skip — you can still scan sites you paste in.</p>
    <div class="row">
      <input type="text" id="apikey" placeholder="Serper API key" style="flex:1;min-width:240px">
      <button onclick="saveSetup()">Save settings</button>
    </div>
    <p id="cfgstate" class="muted" style="margin-bottom:0"></p>
  </div>

  <div class="card">
    <h2>Status</h2>
    <div class="stats">
      <div class="stat"><b id="s-pending">–</b><span>queued</span></div>
      <div class="stat"><b id="s-processing">–</b><span>scanning</span></div>
      <div class="stat"><b id="s-positives">–</b><span>positives</span></div>
      <div class="stat"><b id="s-sites">–</b><span>sites claimed</span></div>
      <div class="stat"><b id="s-awaiting">–</b><span>awaiting review</span></div>
      <div class="stat"><b id="s-jobs">–</b><span>jobs running</span></div>
    </div>
  </div>

  <div class="card">
    <h2>Start / Stop</h2>
    <div class="row" style="margin-bottom:12px">
      <label>Stop after <input type="number" id="positives" min="1" placeholder="∞" style="width:64px"> positives</label>
      <label>Repeat every <input type="number" id="every" min="1" placeholder="once" style="width:72px"> minutes</label>
    </div>
    <div class="row">
      <button class="primary big" onclick="act('/api/auto',true)">▶ Start (find &amp; scan)</button>
      <button class="danger big" onclick="act('/api/stop')">■ Stop</button>
    </div>
    <p id="msg"></p>
  </div>

  <div class="card">
    <h2>More actions</h2>
    <div class="row">
      <button onclick="act('/api/discover')">Discover only</button>
      <button onclick="act('/api/worker',true)">Start a worker</button>
      <button onclick="if(confirm('Clear the queue, claims, positives and stop signal?'))act('/api/reset')">Reset everything</button>
      <button onclick="if(confirm('Shut the control panel down?'))fetch('/api/quit',{method:'POST',headers:H}).then(()=>msg('Panel is shutting down — you can close this tab.'))">Quit panel</button>
    </div>
  </div>

  <div class="card">
    <h2>Scan one site now</h2>
    <div class="row">
      <input type="text" id="one" placeholder="https://suspect-site.com/" style="flex:1;min-width:220px">
      <button class="primary" onclick="scanOne()">Scan it</button>
    </div>
  </div>

  <div class="card">
    <h2>Add several sites to the queue</h2>
    <textarea id="urls" placeholder="https://site-a.com/&#10;https://site-b.com/"></textarea>
    <div class="row" style="margin-top:10px"><button onclick="enqueue()">Add to queue</button></div>
  </div>

  <div class="card">
    <h2>Top hits — review</h2>
    <p class="muted" style="margin-top:0">Open a report, then mark the site. Confirmed sites stay listed; false positives are hidden and never re-queued.</p>
    <div id="hits"><p class="muted">Loading…</p></div>
  </div>

  <div class="card">
    <h2>Activity log</h2>
    <pre id="log" class="log">(nothing yet)</pre>
  </div>
</div>
<script>
  var H={'X-Dorker-Panel':'1'};
  function esc(s){return String(s).replace(/[&<>"']/g,function(c){return{'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]})}
  function band(s){return s>=91?'b-ac':s>=86?'b-vl':s>=76?'b-l':s>=51?'b-p':s>=26?'b-u':'b-vu'}
  function label(s){return s>=91?'Almost certain':s>=86?'Very likely':s>=76?'Likely':s>=51?'Possible':s>=26?'Unlikely':'Very unlikely'}
  function msg(t){var m=document.getElementById('msg');m.textContent=t;setTimeout(function(){if(m.textContent===t)m.textContent='';},4000)}
  function post(path,body){return fetch(path,{method:'POST',headers:H,body:body}).then(r=>r.json())}
  function runQS(){
    var p=document.getElementById('positives').value, e=document.getElementById('every').value, q=[];
    if(p)q.push('positives='+encodeURIComponent(p));
    if(e)q.push('every='+encodeURIComponent(e));
    return q.length?('?'+q.join('&')):'';
  }
  function act(path,useOpts){
    post(path+(useOpts?runQS():'')).then(d=>{msg(d.msg||'Done.');refresh();}).catch(()=>msg('Request failed.'));
  }
  function enqueue(){
    post('/api/enqueue',document.getElementById('urls').value).then(d=>{msg(d.msg||'Queued.');document.getElementById('urls').value='';refresh();}).catch(()=>msg('Request failed.'));
  }
  function scanOne(){
    var u=document.getElementById('one').value.trim();
    if(!u){msg('Enter a URL first.');return;}
    post('/api/scan-url',u).then(d=>{msg(d.msg||'Scanning…');document.getElementById('one').value='';refresh();}).catch(()=>msg('Request failed.'));
  }
  function saveSetup(){
    post('/api/setup',JSON.stringify({api_key:document.getElementById('apikey').value.trim()})).then(d=>{msg(d.msg||'Saved.');refresh();}).catch(()=>msg('Request failed.'));
  }
  function review(host,verdict){
    post('/api/review',JSON.stringify({host:host,verdict:verdict})).then(d=>{msg(d.msg||'Saved.');refresh();}).catch(()=>msg('Request failed.'));
  }
  document.getElementById('hits').addEventListener('click',function(ev){
    var b=ev.target.closest('button[data-verdict]');
    if(b)review(b.getAttribute('data-host'),b.getAttribute('data-verdict'));
  });
  function refresh(){
    fetch('/api/status').then(r=>r.json()).then(d=>{
      document.getElementById('s-pending').textContent=d.pending;
      document.getElementById('s-processing').textContent=d.processing;
      document.getElementById('s-positives').textContent=d.positives;
      document.getElementById('s-sites').textContent=d.sites;
      document.getElementById('s-awaiting').textContent=d.awaiting;
      document.getElementById('s-jobs').textContent=d.jobs;
      document.getElementById('stopbanner').hidden=!d.stop;
      var cs=document.getElementById('cfgstate');
      cs.textContent=d.configured?'Auto-discovery is set up ✓':'Auto-discovery not set up — paste a Serper key above, or just use “Scan one site”.';
      var h=document.getElementById('hits');
      if(!d.hits||!d.hits.length){h.innerHTML='<p class="muted">No hits yet.</p>';return;}
      h.innerHTML=d.hits.map(function(x){
        var host=esc(x.host);
        var link='<a href="'+esc(x.report)+'" target="_blank" rel="noopener">'+esc(x.url)+'</a>';
        var actions=x.review==='confirmed'
          ?'<span class="tag">confirmed</span><button class="small" data-host="'+host+'" data-verdict="clear">Undo</button>'
          :'<button class="small" data-host="'+host+'" data-verdict="confirmed">✓ Confirm</button>'+
           '<button class="small" data-host="'+host+'" data-verdict="false_positive">✗ False positive</button>';
        return '<div class="hit"><span class="badge '+band(x.score)+'">'+x.score+'/100</span>'+
               '<span class="lbl">'+label(x.score)+'</span><span class="u">'+link+'</span><span class="acts">'+actions+'</span></div>';
      }).join('');
    }).catch(()=>{});
    fetch('/api/log').then(r=>r.text()).then(t=>{
      var l=document.getElementById('log'), atEnd=l.scrollTop+l.clientHeight>=l.scrollHeight-4;
      l.textContent=t||'(nothing yet)';
      if(atEnd)l.scrollTop=l.scrollHeight;
    }).catch(()=>{});
  }
  refresh(); setInterval(refresh,3000);
</script>
</body></html>"##;
