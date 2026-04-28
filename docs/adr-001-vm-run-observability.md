# ADR-001: vm run observability — out-of-band Cloudflare logger

**Status:** proposed
**Date:** 2026-04-28

## Context

`utm-dev vm run` launches the built Tauri binary inside the VM and is supposed to tell us whether the app actually starts up cleanly. There are two competing mechanisms:

1. **Capture stdout/stderr from inside the VM.**
   Linux: `xvfb-run` + redirect to `~/.utm-dev-run/run.log`. Windows: `Start-Process -RedirectStandard*` to `%USERPROFILE%\.utm-dev-run\run.log`. Tail via `vm logs --kind run --follow`.

2. **Out-of-band: app's own logger ships events to a Cloudflare Workers endpoint.**
   The Tauri app (the one being built, not utm-dev) embeds a logger that POSTs structured events to a Cloudflare Worker / R2 bucket / Logpush sink at startup. The user has a `wrangler tail` (or browser dashboard URL) open while `vm run` is firing. Startup events arrive there regardless of stdout capture.

Both work in isolation; the question is which one we treat as the canonical observability path.

## Decision

**Default: stdout capture (mechanism 1).** It's zero-config from the user's side — `vm run` just works.

**Preferred when available: out-of-band Cloudflare logger (mechanism 2).** When the app under test ships with the logger, `vm run` doesn't need to capture stdout at all — the logger handles startup observability, and stdout is essentially decorative.

## Why both, not one

- **Stdout capture has known gaps on Windows release builds** — Tauri release apps on Windows don't reliably write to the parent's stdout (subsystem GUI). Even when redirected via `Start-Process -RedirectStandardOutput`, you get nothing useful. So stdout-only is a weak default for Windows specifically.
- **Cloudflare logger has dependencies** — requires Cloudflare account, deployed Worker/endpoint, and a logger crate baked into the app under test. Not every project will have this setup.
- **Out-of-band is strictly more powerful.** When present, it surfaces startup state visible to *any* observer (Claude, the dev, CI), not just the host that ran `vm run`. Useful for cross-machine debugging, AI-assisted diagnosis, and CI gating without log scraping.

## How it composes with utm-dev

`vm run --name X --bin <path>` doesn't (and shouldn't) know whether the app has a Cloudflare logger or not. It just launches and captures stdout. That's mechanism 1.

If the app ships with the Cloudflare logger, the user separately tails the CF endpoint (e.g. `wrangler tail my-app`). `vm run` makes no calls into Cloudflare and has no opinion about it. The logger pattern lives entirely in the app being built — utm-dev is unaware.

Optional sweetener (future): a `vm run --observe <url>` flag that wraps the user's preferred tail command (`wrangler tail`, `curl <url>`, etc.) so the dev sees both stdout AND the CF stream in one terminal. Out of scope for now.

## Logger pattern (informative, not prescriptive)

For repos that want this pattern, a minimal sketch:

```rust
// In the Tauri app, on startup:
let endpoint = env!("CF_LOG_ENDPOINT"); // baked at build time
tauri::async_runtime::spawn(async move {
    let _ = reqwest::Client::new()
        .post(endpoint)
        .json(&serde_json::json!({
            "evt":  "startup",
            "ts":   chrono::Utc::now().to_rfc3339(),
            "ver":  env!("CARGO_PKG_VERSION"),
            "host": std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default(),
        }))
        .send()
        .await;
});
```

The Worker endpoint accumulates events into R2 / Logpush / KV / D1 as the app prefers. `wrangler tail <worker>` streams them in realtime.

## Consequences

- `vm run` stays simple — single responsibility, launch + capture stdout.
- Apps that adopt the CF logger get **better** observability without `vm run` having to know.
- Apps that don't adopt it get the baseline stdout-capture experience. Adequate for most Linux cases; weaker on Windows release builds.
- This ADR documents the *integration point*. Implementing the logger crate itself is the app's concern, not utm-dev's.
