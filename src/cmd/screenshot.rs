//! `utm-dev screenshot` — capture the rendered Tauri WebView via WebDriver.
//!
//! Distinct from `utm-dev vm screenshot` (which uses scrot against Xvfb on
//! a Linux VM and returns a black PNG for WebKit-GTK content). This command
//! runs ON THE HOST against a Tauri project on the host, talking the W3C
//! WebDriver protocol over HTTP to `tauri-webdriver`. Captures actual
//! rendered content.
//!
//! Requires:
//!   - `tauri-webdriver` on PATH (`cargo install tauri-webdriver --locked`)
//!   - The Tauri project must be buildable with `--features webdriver`
//!     (its Cargo.toml exposes the feature; tauri-plugin-webdriver-style
//!     setup; see Tauri docs for the project-side wiring).
//!
//! Ported from `joeblew999/utm-dev/.mise/tasks/screenshot.ts` +
//! `_screenshot.ts`. Same protocol, no behavioural changes — just Rust
//! instead of Bun/TypeScript.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_PORT: u16 = 4444;

pub fn run(out: Option<&str>, port: Option<u16>) -> Result<()> {
    let port = port.unwrap_or(DEFAULT_PORT);
    let project_root = find_tauri_root()?;
    println!("══ Tauri Screenshot ══\n");

    if which::which("tauri-webdriver").is_err() {
        bail!(
            "tauri-webdriver not found on PATH.\n  \
             Install: cargo install tauri-webdriver --locked"
        );
    }

    let session = start_session(&project_root, port)?;

    // Default output path; caller can override.
    let out_path = match out {
        Some(p) => PathBuf::from(p),
        None => project_root.join("screenshots").join("app.png"),
    };
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    println!("→ capturing screenshot...");
    let png = session.capture_screenshot()?;
    std::fs::write(&out_path, &png).with_context(|| format!("writing {}", out_path.display()))?;
    println!(
        "  saved to {} ({:.1} KB)",
        out_path.display(),
        png.len() as f64 / 1024.0
    );

    // Drop runs cleanup
    Ok(())
}

// ── Project root discovery ───────────────────────────────────────────────────

/// Walk up from cwd looking for `src-tauri/tauri.conf.json`.
fn find_tauri_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir().context("cwd")?;
    loop {
        if dir.join("src-tauri").join("tauri.conf.json").is_file() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!(
                "not in a Tauri project (no src-tauri/tauri.conf.json found walking up from cwd)"
            );
        }
    }
}

// ── WebDriver session ────────────────────────────────────────────────────────

struct Session {
    base_url: String,
    session_id: String,
    /// Owned children. Killed on Drop. The proxy first, then the app — matches
    /// startup order and keeps the proxy alive long enough to hand off cleanly.
    procs: Vec<Child>,
}

impl Session {
    fn capture_screenshot(&self) -> Result<Vec<u8>> {
        let url = format!("{}/session/{}/screenshot", self.base_url, self.session_id);
        let resp = ureq::get(&url)
            .call()
            .context("WebDriver GET /screenshot")?;
        let body: Value = read_json(resp).context("parsing /screenshot response")?;
        let b64 = body
            .get("value")
            .and_then(|v| v.as_str())
            .context("/screenshot response missing .value (base64 PNG)")?;
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .context("base64 decoding screenshot payload")
    }
}

/// ureq 3 doesn't expose a direct JSON helper on Body — read text, parse.
fn read_json(resp: ureq::http::Response<ureq::Body>) -> Result<Value> {
    let text = resp
        .into_body()
        .read_to_string()
        .context("reading response body")?;
    serde_json::from_str(&text).with_context(|| {
        format!(
            "parsing JSON: {}",
            text.chars().take(200).collect::<String>()
        )
    })
}

/// POST a serializable body as JSON; ureq 3 takes raw `&str`/`&[u8]` via `.send()`.
fn post_json(url: &str, body: &Value) -> Result<ureq::http::Response<ureq::Body>> {
    let payload = serde_json::to_string(body).context("encoding JSON body")?;
    ureq::post(url)
        .header("Content-Type", "application/json")
        .send(&payload)
        .with_context(|| format!("POST {url}"))
}

impl Drop for Session {
    fn drop(&mut self) {
        for proc in &mut self.procs {
            let _ = proc.kill();
            let _ = proc.wait();
        }
        // Tauri's single-instance plugin leaves /tmp/*_si.sock files around.
        // Without cleanup, the next launch silently exits thinking another
        // instance is running.
        clean_single_instance_sockets();
    }
}

fn clean_single_instance_sockets() {
    let Ok(entries) = std::fs::read_dir("/tmp") else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().ends_with("_si.sock") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Build the Tauri app with `--features webdriver`, launch it, and return a
/// live WebDriver session. Caller drops the Session to clean up.
fn start_session(project_root: &Path, port: u16) -> Result<Session> {
    // Clean any leftover sockets from a prior run.
    clean_single_instance_sockets();

    // Read tauri.conf.json for productName -> binary name.
    let conf_path = project_root.join("src-tauri").join("tauri.conf.json");
    let conf: Value = serde_json::from_str(
        &std::fs::read_to_string(&conf_path)
            .with_context(|| format!("reading {}", conf_path.display()))?,
    )
    .with_context(|| format!("parsing {}", conf_path.display()))?;
    let app_name = conf
        .get("productName")
        .and_then(|v| v.as_str())
        .unwrap_or("app")
        .to_string();

    let manifest = project_root.join("src-tauri").join("Cargo.toml");
    let binary = project_root
        .join("src-tauri")
        .join("target")
        .join("debug")
        .join(&app_name);

    println!("→ building with --features webdriver...");
    let status = Command::new("cargo")
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .args(["--features", "webdriver"])
        .status()
        .context("running cargo build")?;
    if !status.success() {
        bail!("cargo build --features webdriver failed (exit {})", status);
    }
    if !binary.exists() {
        bail!(
            "expected binary not found after build: {}",
            binary.display()
        );
    }
    println!();

    // Launch tauri-webdriver proxy + the app. Children are owned by the
    // returned Session; Drop kills them.
    let mut procs: Vec<Child> = Vec::new();

    println!("→ starting WebDriver on port {port}...");
    procs.push(
        Command::new("tauri-webdriver")
            .args(["--port", &port.to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning tauri-webdriver")?,
    );

    procs.push(
        Command::new(&binary)
            .env("TAURI_WEBVIEW_AUTOMATION", "true")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning {}", binary.display()))?,
    );

    let base_url = format!("http://127.0.0.1:{port}");

    // Poll for session creation. Proxy + app plugin need to both be ready,
    // so this can take 5-30 sec on cold builds.
    let session_id = poll(
        "session",
        Duration::from_secs(45),
        Duration::from_millis(2000),
        || try_create_session(&base_url, &binary),
    )?;
    println!("  session {session_id}");

    // Wait for the page to finish loading.
    poll(
        "page-ready",
        Duration::from_secs(10),
        Duration::from_millis(500),
        || {
            Ok(if is_page_ready(&base_url, &session_id).unwrap_or(false) {
                Some(())
            } else {
                None
            })
        },
    )?;
    println!("  ready\n");

    Ok(Session {
        base_url,
        session_id,
        procs,
    })
}

fn try_create_session(base_url: &str, binary: &Path) -> Result<Option<String>> {
    let body = json!({
        "capabilities": {
            "alwaysMatch": {
                "tauri:options": { "application": binary.to_string_lossy() }
            }
        }
    });
    let resp = match post_json(&format!("{base_url}/session"), &body) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    let v: Value = read_json(resp).unwrap_or(Value::Null);
    let id = v
        .pointer("/value/sessionId")
        .or_else(|| v.pointer("/sessionId"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
    Ok(id)
}

fn is_page_ready(base_url: &str, session_id: &str) -> Result<bool> {
    let body = json!({
        "script": "return document.readyState === 'complete'",
        "args":   [],
    });
    let resp = post_json(
        &format!("{base_url}/session/{session_id}/execute/sync"),
        &body,
    )
    .context("WebDriver POST /execute/sync")?;
    let v: Value = read_json(resp).context("parsing /execute/sync")?;
    Ok(v.get("value").and_then(|x| x.as_bool()).unwrap_or(false))
}

fn poll<T, F>(label: &str, timeout: Duration, interval: Duration, mut f: F) -> Result<T>
where
    F: FnMut() -> Result<Option<T>>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f()? {
            return Ok(v);
        }
        if Instant::now() >= deadline {
            bail!("{label}: timed out after {}s", timeout.as_secs());
        }
        std::thread::sleep(interval);
    }
}
