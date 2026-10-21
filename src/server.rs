//! A small local web control panel: `dorker serve`.
//!
//! Serves a page with buttons (discover, auto, worker, stop, reset, enqueue)
//! and a status view that refreshes itself. Action buttons launch the matching
//! `dorker` command as a child process (sharing the same coordination folder),
//! so the panel is just a friendly front end over the CLI. Bound to localhost.

use crate::coord::Coordinator;
use crate::status;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tiny_http::{Header, Method, Response, Server};

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

    println!("Dorker control panel: http://{addr}");
    println!("(Leave this running. Open that address in your browser. Ctrl+C to stop the panel.)");

    for req in server.incoming_requests() {
        let method = req.method().clone();
        let raw = req.url().to_string();
        let path = raw.split('?').next().unwrap_or("/").to_string();

        match (&method, path.as_str()) {
            (Method::Get, "/") => {
                respond_html(req, PAGE);
            }
            (Method::Get, "/api/status") => {
                respond_json(req, &status_json(coord, output_dir));
            }
            (Method::Post, "/api/discover") => {
                spawn_child(&exe, &coord_dir, &["discover", "--config", &config, "--enqueue"]);
                respond_json(req, r#"{"ok":true,"msg":"Discovery started."}"#);
            }
            (Method::Post, "/api/auto") => {
                let mut args = vec!["auto".to_string(), "--config".into(), config.clone()];
                if let Some(n) = query_num(&raw, "positives") {
                    args.push("--max-positives".into());
                    args.push(n.to_string());
                }
                spawn_child_owned(&exe, &coord_dir, &args);
                respond_json(req, r#"{"ok":true,"msg":"Auto run started."}"#);
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
                spawn_child_owned(&exe, &coord_dir, &args);
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
                    spawn_child(&exe, &coord_dir, &["work", "--config", &config]);
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

fn status_json(coord: &Coordinator, output_dir: &str) -> String {
    let (pending, processing) = coord.queue_counts();
    let positives = coord.positive_count();
    let sites = coord.list_sites().len();
    let stop = coord.stop_requested();
    let hits = status::top_hits(Path::new(output_dir), 10);
    let hits_json: Vec<serde_json::Value> = hits
        .iter()
        .map(|h| {
            let report = format!("/reports/sites/{}/report.html", crate::coord::safe_host(&h.url));
            serde_json::json!({"score": h.score, "url": h.url, "report": report})
        })
        .collect();
    serde_json::json!({
        "pending": pending,
        "processing": processing,
        "positives": positives,
        "sites": sites,
        "stop": stop,
        "hits": hits_json,
    })
    .to_string()
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

fn spawn_child(exe: &Path, coord_dir: &Path, args: &[&str]) {
    let mut cmd = Command::new(exe);
    cmd.arg("--coordinate").arg(coord_dir);
    cmd.args(args);
    let _ = cmd.spawn();
}

fn spawn_child_owned(exe: &Path, coord_dir: &Path, args: &[String]) {
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    spawn_child(exe, coord_dir, &refs);
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
    <h2>Status</h2>
    <div class="stats">
      <div class="stat"><b id="s-pending">–</b><span>queued</span></div>
      <div class="stat"><b id="s-processing">–</b><span>scanning</span></div>
      <div class="stat"><b id="s-positives">–</b><span>positives</span></div>
      <div class="stat"><b id="s-sites">–</b><span>sites claimed</span></div>
    </div>
  </div>

  <div class="card">
    <h2>Start / Stop</h2>
    <div class="row" style="margin-bottom:12px">
      <label>Stop after <input type="number" id="positives" min="1" placeholder="∞" style="width:64px"> positives</label>
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
      <button onclick="if(confirm('Shut the control panel down?'))fetch('/api/quit',{method:'POST'}).then(()=>msg('Panel is shutting down — you can close this tab.'))">Quit panel</button>
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
    <h2>Top hits</h2>
    <div id="hits"><p class="muted">Loading…</p></div>
  </div>
</div>
<script>
  function band(s){return s>=91?'b-ac':s>=86?'b-vl':s>=76?'b-l':s>=51?'b-p':s>=26?'b-u':'b-vu'}
  function label(s){return s>=91?'Almost certain':s>=86?'Very likely':s>=76?'Likely':s>=51?'Possible':s>=26?'Unlikely':'Very unlikely'}
  function msg(t){var m=document.getElementById('msg');m.textContent=t;setTimeout(function(){if(m.textContent===t)m.textContent='';},4000)}
  function positivesQS(){var v=document.getElementById('positives').value;return v?('?positives='+encodeURIComponent(v)):''}
  function act(path,usePositives){
    fetch(path+(usePositives?positivesQS():''),{method:'POST'}).then(r=>r.json()).then(d=>{msg(d.msg||'Done.');refresh();}).catch(()=>msg('Request failed.'));
  }
  function enqueue(){
    var body=document.getElementById('urls').value;
    fetch('/api/enqueue',{method:'POST',body:body}).then(r=>r.json()).then(d=>{msg(d.msg||'Queued.');document.getElementById('urls').value='';refresh();}).catch(()=>msg('Request failed.'));
  }
  function scanOne(){
    var u=document.getElementById('one').value.trim();
    if(!u){msg('Enter a URL first.');return;}
    fetch('/api/scan-url',{method:'POST',body:u}).then(r=>r.json()).then(d=>{msg(d.msg||'Scanning…');document.getElementById('one').value='';refresh();}).catch(()=>msg('Request failed.'));
  }
  function refresh(){
    fetch('/api/status').then(r=>r.json()).then(d=>{
      document.getElementById('s-pending').textContent=d.pending;
      document.getElementById('s-processing').textContent=d.processing;
      document.getElementById('s-positives').textContent=d.positives;
      document.getElementById('s-sites').textContent=d.sites;
      document.getElementById('stopbanner').hidden=!d.stop;
      var h=document.getElementById('hits');
      if(!d.hits||!d.hits.length){h.innerHTML='<p class="muted">No hits yet.</p>';return;}
      h.innerHTML=d.hits.map(function(x){
        var safe=x.url.replace(/</g,'&lt;');
        var link=x.report?('<a href="'+x.report+'" target="_blank" rel="noopener">'+safe+'</a>'):('<span class="muted">'+safe+'</span>');
        return '<div class="hit"><span class="badge '+band(x.score)+'">'+x.score+'/100</span>'+
               '<span>'+label(x.score)+'</span>'+link+'</div>';
      }).join('');
    }).catch(()=>{});
  }
  refresh(); setInterval(refresh,3000);
</script>
</body></html>"##;
