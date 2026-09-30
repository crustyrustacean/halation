//! End-to-end smoke test in a real browser.
//!
//! Why this exists: the app's interactive behaviour is Datastar's, and
//! Datastar runs in the browser. A server-side test can assert that
//! `/fragments/feed` returns a well-formed SSE stream — and it did, all green,
//! for months — while the deployed "Load more photos" button did nothing at
//! all, because the vendored `datastar.js` is a *library* build that binds no
//! handlers until something calls `start()`. No HTTP-level assertion could
//! have caught that; only a real click could.
//!
//! So: drive a real Chromium, click the real button, count the posts.
//!
//! Deliberately dependency-free, like the rest of xtask. Puppeteer would mean
//! a node toolchain and a `package.json` in a Rust repo; instead this launches
//! Chrome with `--remote-debugging-port` and speaks the DevTools Protocol over
//! a hand-rolled WebSocket client built on `std::net::TcpStream`.
//!
//! Usage:
//!   cargo xtask e2e                    # start the app, drive it, shut down
//!   cargo xtask e2e --url <url>        # drive an already-running instance
//!   cargo xtask e2e --keep             # leave the app running afterwards

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DEBUG_PORT: u16 = 9222;
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
const APP_TIMEOUT: Duration = Duration::from_secs(120);
const POST_CLICK_WAIT: Duration = Duration::from_secs(4);

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let mut url: Option<String> = None;
    let mut keep = false;
    let mut db: Option<String> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--url" => {
                url = Some(
                    iter.next()
                        .ok_or("--url needs a value, e.g. --url http://localhost:8000")?
                        .clone(),
                );
            }
            "--db" => {
                db = Some(
                    iter.next()
                        .ok_or("--db needs a database name, e.g. --db e2e_probe")?
                        .clone(),
                );
            }
            "--keep" => keep = true,
            other => {
                return Err(format!(
                    "unknown option: {other}. Usage: cargo xtask e2e [--url <url>] [--db <name>] [--keep]"
                ));
            }
        }
    }

    let browser = find_browser().ok_or_else(|| {
        "no Chromium-family browser found. Install Chrome or Edge, or pass --url to drive \
         an already-running instance."
            .to_string()
    })?;
    println!("browser: {}", browser.display());

    // A supplied --url means the caller owns the lifecycle.
    let mut app: Option<Child> = None;
    let target = match url {
        Some(u) => u,
        None => {
            let port = free_port()?;
            let base = format!("http://127.0.0.1:{port}");
            println!("app:     {base} (memory storage, no external calls)");
            app = Some(spawn_app(root, port, db.as_deref())?);
            wait_for_health(&base, APP_TIMEOUT)?;
            base
        }
    };

    let result = drive(&browser, &target);

    if let Some(mut child) = app {
        if keep {
            println!("\n--keep: the app is still running on {target}; stop it when you're done.");
        } else {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    result
}

// --- the browser and the app ----------------------------------------------

fn find_browser() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = if cfg!(windows) {
        [
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    } else if cfg!(target_os = "macos") {
        vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into(),
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into(),
        ]
    } else {
        vec![
            "/usr/bin/google-chrome".into(),
            "/usr/bin/chromium".into(),
            "/usr/bin/chromium-browser".into(),
            "/usr/bin/microsoft-edge".into(),
        ]
    };
    candidates.into_iter().find(|p| p.exists())
}

fn spawn_app(root: &Path, port: u16, db: Option<&str>) -> Result<Child, String> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root)
        .args(["run", "--bin", "halation"])
        // Memory storage keeps this from touching real object storage — the
        // same guarantee `cargo test` now enforces.
        .env("APP_STORAGE__BACKEND", "memory")
        .env("APP_ENVIRONMENT", "local")
        .env("APP_APPLICATION__PORT", port.to_string());
    if let Some(db) = db {
        cmd.env("APP_DATABASE__DATABASE_NAME", db);
    }
    cmd.stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("failed to launch the app: {e}"))
}

fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("could not find a free port: {e}"))
}

fn wait_for_health(base: &str, timeout: Duration) -> Result<(), String> {
    let health = format!("{base}/api/v1/health");
    let deadline = Instant::now() + timeout;
    let mut last = "no attempt yet".to_string();
    while Instant::now() < deadline {
        match http_get(&health) {
            Ok((200, _)) => return Ok(()),
            Ok((status, _)) => last = format!("status {status}"),
            Err(e) => last = e,
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!(
        "the app never became healthy within {timeout:?} (last: {last}). \
         Is the dev database up? Try `cargo xtask dev-db`."
    ))
}

// --- a minimal HTTP client (no deps) --------------------------------------

fn http_get(url: &str) -> Result<(u16, String), String> {
    let (host, port, path) = split_url(url)?;
    let mut stream = TcpStream::connect((host.as_str(), port)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept: */*\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;

    // Chrome does not honour `Connection: close` here: it holds the socket
    // open, so reading to EOF would block until the read timeout and report a
    // spurious connection error. The response carries a Content-Length, so
    // read until the body is complete and stop.
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if Instant::now() >= deadline {
            if raw.is_empty() {
                return Err(format!("reading {url} timed out with nothing received"));
            }
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&chunk[..n]);
                if body_is_complete(&raw) {
                    break;
                }
            }
            Err(e) => {
                if raw.is_empty() {
                    return Err(format!("reading {url} failed: {e}"));
                }
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            format!(
                "no status line in response: {}",
                text.lines().next().unwrap_or("")
            )
        })?;
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Ok((status, body))
}

/// True once a Content-Length-delimited response has arrived in full, or a
/// chunked one has seen its terminating marker.
fn body_is_complete(raw: &[u8]) -> bool {
    let text = String::from_utf8_lossy(raw);
    let Some(split) = text.find("\r\n\r\n") else {
        return false;
    };
    let (headers, body) = text.split_at(split);
    let body = &body[4..];

    if headers
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        return body.ends_with("0\r\n\r\n");
    }
    for line in headers.lines() {
        if let Some(value) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            if let Ok(expected) = value.trim().parse::<usize>() {
                return body.len() >= expected;
            }
        }
    }
    // No length and no chunking: the server will close when it is done.
    false
}

fn split_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("only http:// URLs are supported, got {url}"))?;
    let (host_port, path) = match rest.find('/') {
        Some(i) => (rest[..i].to_string(), rest[i..].to_string()),
        None => (rest.to_string(), "/".to_string()),
    };
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>().map_err(|_| format!("bad port in {url}"))?,
        ),
        None => (host_port, 80),
    };
    Ok((host, port, path))
}

// --- a minimal WebSocket client (RFC 6455), enough for CDP ----------------

struct WebSocket {
    stream: TcpStream,
    next_id: u64,
}

impl WebSocket {
    fn connect(url: &str) -> Result<Self, String> {
        let rest = url
            .strip_prefix("ws://")
            .ok_or_else(|| format!("only ws:// is supported, got {url}"))?;
        let (host_port, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (host, port) = match host_port.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().map_err(|e| e.to_string())?),
            None => (host_port, 80),
        };

        let mut stream = TcpStream::connect((host, port)).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| e.to_string())?;

        // Any base64 of 16 bytes satisfies the server's check; the key exists
        // to prove we are not a cache-poisoning intermediary.
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n"
        )
        .map_err(|e| e.to_string())?;

        let mut header = Vec::new();
        let mut byte = [0u8; 1];
        while !header.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte).map_err(|e| e.to_string())? == 0 {
                return Err("connection closed during the websocket handshake".to_string());
            }
            header.push(byte[0]);
        }
        let header_text = String::from_utf8_lossy(&header).into_owned();
        if !header_text.contains(" 101 ") {
            return Err(format!(
                "websocket handshake refused: {}",
                header_text.lines().next().unwrap_or("")
            ));
        }

        Ok(Self { stream, next_id: 0 })
    }

    /// Send a CDP command and return the `result` object of its response.
    /// Events and responses to other ids are discarded.
    fn call(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: &str,
    ) -> Result<String, String> {
        self.next_id += 1;
        let id = self.next_id;
        let message = match session {
            Some(s) => format!(
                "{{\"id\":{id},\"sessionId\":{},\"method\":{},\"params\":{params}}}",
                quote(s),
                quote(method)
            ),
            None => format!(
                "{{\"id\":{id},\"method\":{},\"params\":{params}}}",
                quote(method)
            ),
        };
        self.write_frame(message.as_bytes())?;

        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let frame = self.read_frame()?;
            if !frame.contains(&format!("\"id\":{id}")) {
                continue; // an event, or someone else's response
            }
            if let Some(error) = json_field(&frame, "error") {
                return Err(format!("{method} failed: {error}"));
            }
            return Ok(json_field(&frame, "result").unwrap_or_default());
        }
        Err(format!("no response to {method} within 30s"))
    }

    fn write_frame(&mut self, payload: &[u8]) -> Result<(), String> {
        let mut frame = vec![0x81u8]; // FIN + text opcode
        let mask = [0x37u8, 0xfa, 0x21, 0x3d];
        let len = payload.len();
        if len < 126 {
            frame.push(0x80 | len as u8);
        } else if let Ok(short) = u16::try_from(len) {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&short.to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
        frame.extend_from_slice(&mask);
        for (i, byte) in payload.iter().enumerate() {
            frame.push(byte ^ mask[i % 4]);
        }
        self.stream.write_all(&frame).map_err(|e| e.to_string())?;
        self.stream.flush().map_err(|e| e.to_string())
    }

    fn read_frame(&mut self) -> Result<String, String> {
        // The first byte is FIN plus the opcode nibble; the mask flag lives in
        // the *second* byte, alongside the payload length.
        let first = self.read_byte()?;
        let opcode = first & 0x0f;
        if opcode == 0x8 {
            return Err("the browser closed the connection".to_string());
        }
        if opcode != 0x1 {
            return Err(format!("unexpected websocket opcode {opcode}"));
        }

        let second = self.read_byte()?;
        if second & 0x80 != 0 {
            return Err("the server must not mask its frames".to_string());
        }
        let mut length = (second & 0x7f) as usize;
        if length == 126 {
            let mut b = [0u8; 2];
            self.stream.read_exact(&mut b).map_err(|e| e.to_string())?;
            length = u16::from_be_bytes(b) as usize;
        } else if length == 127 {
            let mut b = [0u8; 8];
            self.stream.read_exact(&mut b).map_err(|e| e.to_string())?;
            length = u64::from_be_bytes(b) as usize;
        }

        let mut payload = vec![0u8; length];
        self.stream
            .read_exact(&mut payload)
            .map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&payload).into_owned())
    }

    fn read_byte(&mut self) -> Result<u8, String> {
        let mut b = [0u8; 1];
        self.stream.read_exact(&mut b).map_err(|e| e.to_string())?;
        Ok(b[0])
    }
}

/// Quote a Rust string as a JSON string.
fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Pull one field out of a flat JSON object. Adequate for CDP replies, which
/// are small and whose nested values we only ever need whole.
///
/// This is deliberately a scanner rather than a real parser, so it has to
/// balance brackets when a value is an object or array. Skipping that step
/// silently truncates a nested value at its first `}` — which looks like a
/// short, plausible reply and is very hard to spot from the outside.
fn json_field(haystack: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = haystack.find(&needle)? + needle.len();
    // Chrome pretty-prints its devtools JSON, so there is whitespace between
    // the colon and the value. Skip it before deciding what type we have.
    let rest = haystack[start..].trim_start();

    if let Some(stripped) = rest.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = stripped.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => break,
                },
                c => out.push(c),
            }
        }
        return None;
    }

    // A nested object or array: balance brackets so the value comes back
    // whole. A scalar runs to the next comma or closing brace.
    let opening = rest.chars().next().unwrap_or('}');
    if matches!(opening, '{' | '[') {
        let closing = if opening == '{' { '}' } else { ']' };
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        for (i, c) in rest.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match c {
                '\\' if in_string => escaped = true,
                '"' => in_string = !in_string,
                _ if in_string => {}
                c if c == opening => depth += 1,
                c if c == closing => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(rest[..i + 1].to_string());
                    }
                }
                _ => {}
            }
        }
        return Some(rest.to_string());
    }
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

// --- the test itself -------------------------------------------------------

fn drive(browser: &Path, url: &str) -> Result<(), String> {
    let profile = std::env::temp_dir().join(format!("halation-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile);

    let mut child = Command::new(browser)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-extensions")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--remote-debugging-port={DEBUG_PORT}"))
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("failed to launch the browser: {e}"))?;

    let result = drive_inner(url);

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&profile);
    result
}

/// Open a DevTools connection to the already-running browser.
fn connect() -> Result<WebSocket, String> {
    // Wait for the debugging endpoint to come up.
    let deadline = Instant::now() + LAUNCH_TIMEOUT;
    let ws_url = loop {
        match devtools_url() {
            Ok(u) => break u,
            Err(e) => {
                if Instant::now() >= deadline {
                    return Err(format!("the browser never exposed devtools: {e}"));
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    };
    WebSocket::connect(&ws_url)
}

/// Open a fresh tab, attach to it, and return the session id.
fn new_page(ws: &mut WebSocket, url: &str) -> Result<String, String> {
    let created = ws.call(
        None,
        "Target.createTarget",
        &format!("{{\"url\":{}}}", quote("about:blank")),
    )?;
    let target_id = json_field(&created, "targetId")
        .ok_or_else(|| format!("Target.createTarget returned no targetId: {created}"))?;
    let attached = ws.call(
        None,
        "Target.attachToTarget",
        &format!("{{\"targetId\":{},\"flatten\":true}}", quote(&target_id)),
    )?;
    let session = json_field(&attached, "sessionId")
        .ok_or_else(|| format!("Target.attachToTarget returned no sessionId: {attached}"))?;
    ws.call(Some(&session), "Page.enable", "{}")?;
    ws.call(Some(&session), "Runtime.enable", "{}")?;
    ws.call(Some(&session), "Log.enable", "{}")?;
    if !url.is_empty() {
        ws.call(
            Some(&session),
            "Page.navigate",
            &format!("{{\"url\":{}}}", quote(url)),
        )?;
    }
    Ok(session)
}

fn drive_inner(url: &str) -> Result<(), String> {
    let mut ws = connect()?;
    let session = new_page(&mut ws, url)?;

    // Anything the page logged or threw *during load* is collected before we
    // get in the way, since these events arrive before our first call returns.
    let load_log = drain_events(&mut ws);
    if std::env::var("HALATION_E2E_DEBUG").is_ok() {
        for line in &load_log {
            eprintln!("load: {line}");
        }
    }
    wait_for_ready(&mut ws, &session)?;

    // One self-diagnosing probe: confirm the Datastar module actually parsed,
    // because a module served with a syntax error is discarded silently and
    // leaves every attribute inert with an empty console.
    let module_ok = eval(
        &mut ws,
        &session,
        "(async () => {\
           try {\
             const m = await import('/static/datastar.js');\
             return 'ok, ' + Object.keys(m).length + ' exports';\
           } catch (e) {\
             return 'FAILED TO PARSE: ' + e.message;\
           }\
         })()",
    )?;
    if module_ok.starts_with("FAILED") {
        return Err(format!(
            "the Datastar module did not load ({module_ok}).\n\
             Until it parses, no data-on:* attribute in the app can work."
        ));
    }
    println!("datastar module: {module_ok}");

    let before = post_count(&mut ws, &session)?;
    println!("posts before click: {before}");

    if !has_button(&mut ws, &session)? {
        println!("no load-more button rendered — the feed fits on one page, nothing to click.");
        return Ok(());
    }

    // The real interaction: a genuine click event that Datastar's delegated
    // listener must catch, exactly as a person's click would.
    ws.call(Some(&session), "Network.enable", "{}")?;
    ws.call(Some(&session), "Runtime.enable", "{}")?;
    click(&mut ws, &session)?;
    std::thread::sleep(POST_CLICK_WAIT);

    // What did the click actually do? Measure, do not assume — a hardcoded
    // diagnosis in the error message is a guess wearing a suit.
    let activity = drain_events(&mut ws);
    if std::env::var("HALATION_E2E_DEBUG").is_ok() {
        for line in &activity {
            eprintln!("event: {line}");
        }
    }

    let after = post_count(&mut ws, &session)?;
    println!("posts after click:  {after}");

    if after > before {
        println!("\nPASS: the click loaded {} more post(s).", after - before);
        return Ok(());
    }

    Err(format!(
        "FAIL: clicking load-more changed nothing ({before} -> {after} posts).\n\
         Browser activity while waiting: {}\n\n\
         The module parses, so Datastar is loaded. The usual cause is attribute\n\
         syntax: this bundle splits plugin from key on a COLON, so it must be\n\
         data-on:click. A data-on-click attribute is silently dropped.",
        if activity.is_empty() {
            "none — no request to /fragments/ and no console output".to_string()
        } else {
            activity.join(" | ")
        }
    ))
}

/// Collect whatever the browser emitted while we waited: network requests to
/// our own endpoints, and any console or exception output.
fn drain_events(ws: &mut WebSocket) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_millis(700);
    let mut out = Vec::new();
    while Instant::now() < deadline {
        let _ = ws.stream.set_read_timeout(Some(Duration::from_millis(120)));
        let frame = match ws.read_frame() {
            Ok(f) => f,
            // A read timeout just means nothing is pending, not that the
            // connection is broken. Keep waiting until the overall deadline.
            Err(_) => continue,
        };
        if frame.contains("\"id\":") {
            continue; // a reply to one of our own calls
        }
        if let Some(url) = json_field(&frame, "url") {
            if url.contains("/fragments/") || url.contains("datastar") {
                out.push(format!("REQUEST {url}"));
            }
        }
        if let Some(text) = json_field(&frame, "text") {
            let t: String = text.chars().take(220).collect();
            if !t.trim().is_empty() {
                out.push(format!("CONSOLE {t}"));
            }
        }
        if let Some(details) = json_field(&frame, "exceptionDetails") {
            // The description is the useful half; `text` is often just
            // "Uncaught", which tells you nothing.
            let described = json_field(&details, "description")
                .or_else(|| Some(details.clone()))
                .unwrap_or_default();
            out.push(format!(
                "EXCEPTION {}",
                &described[..described.len().min(400)]
            ));
        }
    }
    // Hand the socket back with a sane timeout, or the next call() times out
    // against the 120ms poll window we installed above.
    let _ = ws.stream.set_read_timeout(Some(Duration::from_secs(30)));
    out
}

fn wait_for_ready(ws: &mut WebSocket, session: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = String::new();
    while Instant::now() < deadline {
        match eval(ws, session, "String(document.readyState)") {
            Ok(state) => {
                if state == "complete" {
                    return Ok(());
                }
                last = state;
            }
            Err(e) => last = e,
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "the page never finished loading (last readyState: {last})"
    ))
}

/// Evaluate an expression and return its value, rendered as text.
fn eval(ws: &mut WebSocket, session: &str, expression: &str) -> Result<String, String> {
    let result = ws.call(
        Some(session),
        "Runtime.evaluate",
        &format!(
            "{{\"expression\":{},\"returnByValue\":true,\"awaitPromise\":true}}",
            quote(expression)
        ),
    )?;
    // A thrown expression comes back under `exceptionDetails`, not as an error.
    if let Some(details) = json_field(&result, "exceptionDetails") {
        return Err(format!(
            "the page threw evaluating {expression:?}: {details}"
        ));
    }
    if std::env::var("HALATION_E2E_DEBUG").is_ok() {
        eprintln!("cdp eval: {expression} -> {result}");
    }
    Ok(json_field(&result, "value").unwrap_or_default())
}

fn devtools_url() -> Result<String, String> {
    let (status, body) = http_get(&format!("http://127.0.0.1:{DEBUG_PORT}/json/version"))?;
    if status != 200 {
        return Err(format!("devtools returned {status}"));
    }
    let mut url = json_field(&body, "webSocketDebuggerUrl").ok_or_else(|| {
        format!(
            "no webSocketDebuggerUrl in: {}",
            &body[..body.len().min(200)]
        )
    })?;

    // Chrome 154 reports the browser endpoint as `ws://127.0.0.1/devtools/…`
    // with no port at all, assuming the client learned it from the HTTP
    // request it just made. We know it, so put it back.
    if let Some(rest) = url.strip_prefix("ws://127.0.0.1/") {
        url = format!("ws://127.0.0.1:{DEBUG_PORT}/{rest}");
    }
    if !url.contains(&format!(":{DEBUG_PORT}")) {
        eprintln!("devtools: raw url was {url:?} (rewritten to {url:?})");
    }
    Ok(url)
}

fn post_count(ws: &mut WebSocket, session: &str) -> Result<usize, String> {
    eval(
        ws,
        session,
        "String(document.querySelectorAll('article.post').length)",
    )?
    .parse()
    .map_err(|e| format!("post count was not a number: {e}"))
}

fn has_button(ws: &mut WebSocket, session: &str) -> Result<bool, String> {
    Ok(eval(
        ws,
        session,
        "String(!!document.querySelector('#load-more'))",
    )? == "true")
}

fn click(ws: &mut WebSocket, session: &str) -> Result<(), String> {
    eval(
        ws,
        session,
        "(() => { const b = document.querySelector('#load-more'); if (!b) throw new Error('no #load-more'); b.click(); return 'clicked'; })()",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{body_is_complete, json_field, split_url};

    #[test]
    fn nested_object_comes_back_whole() {
        // A nested value truncated at its first `}` looks like a short,
        // plausible reply, so pin the whole thing explicitly.
        let frame = r#"{"id":7,"result":{"type":"string","value":"complete"}}"#;
        assert_eq!(
            json_field(frame, "result").as_deref(),
            Some(r#"{"type":"string","value":"complete"}"#)
        );
        assert_eq!(json_field(frame, "id").as_deref(), Some("7"));
    }

    #[test]
    fn braces_inside_strings_do_not_confuse_depth() {
        let frame = r#"{"result":{"value":"a } brace","n":1},"id":2}"#;
        assert_eq!(
            json_field(frame, "result").as_deref(),
            Some(r#"{"value":"a } brace","n":1}"#)
        );
    }

    #[test]
    fn escaped_quote_inside_a_string() {
        let frame = r#"{"result":{"value":"say \"hi\""},"id":3}"#;
        assert_eq!(
            json_field(frame, "result").as_deref(),
            Some(r#"{"value":"say \"hi\""}"#)
        );
    }

    #[test]
    fn arrays_are_balanced_too() {
        let frame = r#"{"result":[1,2,{"deep":true}],"id":4}"#;
        assert_eq!(
            json_field(frame, "result").as_deref(),
            Some(r#"[1,2,{"deep":true}]"#)
        );
    }

    #[test]
    fn whitespace_after_the_colon_is_skipped() {
        // Chrome pretty-prints its devtools JSON.
        let pretty = "{\n  \"webSocketDebuggerUrl\": \"ws://127.0.0.1/devtools/browser/x\"\n}";
        assert_eq!(
            json_field(pretty, "webSocketDebuggerUrl").as_deref(),
            Some("ws://127.0.0.1/devtools/browser/x")
        );
    }

    #[test]
    fn http_url_splits_into_host_port_and_path() {
        let (host, port, path) = split_url("http://127.0.0.1:9222/json/version").expect("splits");
        assert_eq!(
            (host.as_str(), port, path.as_str()),
            ("127.0.0.1", 9222, "/json/version")
        );
    }

    #[test]
    fn a_ws_url_is_rejected_by_the_http_helper() {
        // split_url feeds http_get only. The websocket URL is handled by hand
        // because Chrome omits the port from it.
        assert!(split_url("ws://127.0.0.1/devtools/browser/abc").is_err());
    }

    #[test]
    fn content_length_response_is_complete_only_once_whole() {
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab";
        assert!(!body_is_complete(head.as_bytes()));
        let whole = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabcde";
        assert!(body_is_complete(whole.as_bytes()));
    }

    #[test]
    fn chunked_response_needs_its_terminator() {
        let partial = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n";
        assert!(!body_is_complete(partial.as_bytes()));
        let done = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        assert!(body_is_complete(done.as_bytes()));
    }
}
