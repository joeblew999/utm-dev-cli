# Gap Analysis — utm-dev-cli

Punch list of what's missing or rough. Triaged by impact.

## Source-of-truth invariant

**Rust + tauri-cli + bun + node etc. are pinned by the user's project `mise.toml` — utm-dev's bootstrap does NOT install language runtimes.** The bootstrap only installs *non-mise-managed* prerequisites (apt deps, VS Build Tools, OpenSSH, WebView2). Anything mise can manage stays in mise's hands.

This is why a project consuming utm-dev MUST have `[tools] rust = "..."` (and `cargo:tauri-cli`, `bun`, etc.) declared in its `mise.toml` — `vm build` runs `mise install` inside the project dir to provision exactly those.

## Validated end-to-end

The full pipeline is **proven against [utm-dev-demo](https://github.com/joeblew999/utm-dev-demo)** (vanilla Tauri 2 starter):

| Phase | Linux ARM64 | Windows x86_64 |
|---|---|---|
| `vm up` (idempotent re-run) | ~5 sec | ~30 sec |
| `vm build` first run | ~14 min | ~7 min cargo + ~18 min mise install |
| `vm build` cached re-run | unmeasured (similar) | **2 min 39 sec** |
| Artifacts pulled to `.build/<platform>/<arch>/` | `.deb` 3.8 MB, `.rpm` 3.8 MB, `.AppImage` 75 MB | `.msi` 2.8 MB, `-setup.exe` 1.8 MB |
| `vm run --bin <auto>` | ✓ Xvfb + openbox + GUI app launches | ✓ launches; see Win runtime caveat |
| `vm screenshot` | ✓ Linux only — black png on Tauri (WebKit-GTK needs GL; Xvfb has none) | n/a (Win headless render isn't supported via SSH) |

## Functional gaps

1. **Windows ARM64 native build — BLOCKED on Microsoft.**
   VS Build Tools on ARM64 hosts ships `Hostarm64\x64` and `Hostarm64\x86` cross-tools but no `Hostarm64\arm64` (native ARM64 toolchain). `vs_buildtools.exe --add Microsoft.VisualStudio.Component.VC.Tools.ARM64` and `vs_installer.exe modify --add ...` both return exit 0 without installing the component. Microsoft appears to not yet ship a native ARM64-host-targeting-ARM64 MSVC toolchain.
   **Workaround in place:** cross-compile x86_64 from ARM64 (Hostarm64\x64), runs under Windows ARM64 emulation. x86_64 is what most Windows users actually ship anyway. Re-test periodically as MSVC catches up.

2. **Windows VM C: drive runs out of disk.**
   The Vagrant `utm/windows-11` box ships with a 26 GB C: drive. VS Build Tools eats ~5 GB, Windows + WebView2 + bootstrapper leftovers another ~15 GB — leaving ~0.5 GB free. Symptoms: Tauri release apps can fail to start; bundlers may run out of space mid-build.
   **Workarounds:**
   - `utm-dev vm clean --name windows-build` reclaims a few hundred MB (DISM cleanup, removes installer leftovers).
   - `utm-dev vm resize-disk --name windows-build --plus-gb 30` grows the qcow2 + extends the partition. The cargo target dir is already on D: (the additional 60 GB drive in this box) which is fine — only C: is constrained.
   - Long-term: rebuild the Vagrant box with a larger primary disk and publish it as a pre-baked URL via [docs/box-publishing.md](docs/box-publishing.md).

3. **Tauri Windows release builds exit silently in headless `vm run`.**
   Tauri Windows release apps use the GUI subsystem (no stdout) and exit immediately when launched in a non-interactive desktop session. `vm run --name windows-build` returns a PID but the app then exits before doing anything observable. **Not a utm-dev bug** — it's how Win32 GUI subsystem + headless SSH session interact. For Windows visual verification:
   - RDP into the VM (port `3389` forwarded — `mstsc /v:127.0.0.1:3389` from Windows or use any RDP client to `localhost:3389`).
   - Or have the Tauri app embed an out-of-band logger (Cloudflare Worker, etc.) per [docs/adr-001-vm-run-observability.md](docs/adr-001-vm-run-observability.md).

4. **`vm screenshot` against Tauri Linux returns a black PNG.**
   Bare Xvfb has no GL backend (DRI3 unavailable). WebKit-GTK content (the WebView Tauri renders into) requires GL/EGL and silently doesn't paint. Xvfb + openbox correctly maps the window frame; just the WebKit content area is empty. Process-level verification (`pgrep`, run.log) works fine. For visual capture, dev should:
   - Use the Cloudflare-logger pattern (ADR-001) for app-level observability.
   - Or VNC into the VM with a real GL stack.

## Future direction

**Expose utm-dev as an MCP server via [turbomcp](https://github.com/Epistates/turbomcp).** The CLI surface (vm up/down/build/exec/logs/...) maps cleanly onto MCP tools. Devs and AI assistants would then drive cross-platform Tauri builds via standard MCP tooling instead of shelling out. Keep CLI as the underlying engine; MCP is a thin adapter on top.

**Dogfood loop:** [turbomcpstudio](https://github.com/Epistates/turbomcpstudio) is itself a Tauri app that wraps turbomcp — so we build it (cross-platform) *with utm-dev*, in order to ship the GUI that talks to the MCP server we'll later expose *from* utm-dev. utm-dev has to be reliable enough to build a non-trivial Tauri app (turbomcpstudio is a real validator, not a vanilla starter). Treat its first successful Windows + Linux build as the readiness milestone before MCP work starts.

---

## Recently resolved

- **End-to-end validation against utm-dev-demo** — Windows .msi/.exe + Linux .deb/.rpm/.AppImage all produced and pulled to host. Subsequent Windows build: 2m 39s (cached). vm run + vm logs work on both.
- **`vm clean`** — clears VM-side build/run logs, installer leftovers, runs DISM cleanup on Windows.
- **`vm restart`, `vm doctor`, `vm screenshot`, `vm logs --tail/--errors`, `vm run --bin <auto>`** — full set of dev-loop ergonomics shipped.
- **Pre-baked box pipeline** — `prebaked_url: Option<&str>` on VmProfile + `download_prebaked` resume-aware fetcher. Onboarding goes from ~30 min bootstrap to ~5 min download. See [docs/box-publishing.md](docs/box-publishing.md).
- **`MISE_CARGO_BINSTALL=true`** + cargo-binstall pre-installed by bootstrap (Linux + Windows) — drops per-project tauri-cli install from a 25-min compile to a ~30-sec download.
- **sccache wrapping** — best-effort cross-project artifact cache via RUSTC_WRAPPER + `cargo:sccache` from mise. Big win after the first compile of common Tauri lib deps.
- **GitHub Actions release workflow** — tag `vX.Y.Z` produces a mac-arm64 binary; devs install via `mise use ubi:joeblew999/utm-dev-cli@latest`.
- **Auto-error-dump on `vm build` failure** — bail prints the error stanza tail before the exit; no need to run `vm logs` after.
- **`vm build` mise.toml pre-flight** — bails in 50 ms if rust + cargo:tauri-cli aren't pinned, instead of 25 min into a doomed VM run.
- **Per-phase elapsed timing in `vm build`** — `⌚ sync: 3s | mise install: 8m | cargo tauri build: 22m | total: 30m`.
- **Cross-compile x86_64 on Windows from ARM64 VM** — `--target x86-64` ships clean. Linux x86_64 cross via Debian multiarch + `gcc-x86-64-linux-gnu` + linker env, on demand.
- **`winrm::run_elevated` polling hang** — sentinel-file completion detection.
- **CARGO_TARGET_DIR probe** — fenced `BEGIN_CTD/END_CTD` markers.
- **WebView2 install via Evergreen Bootstrapper** — winget Microsoft.EdgeWebView2Runtime is unreliable on fresh Vagrant boxes; switched to MS's documented headless installer.
- **`cd /d` for Windows bundle archive** — bare `cd` doesn't switch drives in cmd.exe; D: target dir was being missed.
- **vm run process detachment on Linux** — bypassed libssh2 + `-tt` (both kill backgrounded children); use direct ssh subprocess + setsid -f. `pkill <name>` (not `-f`) to avoid pkill matching its own shell's command line.
- **Windows vm run PowerShell single-line** — cmd's `^<nl>` continuation doesn't survive SSH delivery.
- **Dead code cleanup** — many removed; see git log.

## PowerShell quoting in switch_rustup install step (2026-05-02)

The `switch_rustup` PowerShell call in `src/vm/build.rs` (in the Windows
`mise install` step) fails with `MissingEndCurlyBrace` when the second
`powershell -NoProfile -Command "..."` call is chained via `&&`. cmd.exe's
quote handling combined with PowerShell's own quoting confuses things.

Discovered while running `utm-dev windows build --release` against
utm-dev-cli itself (plain-cargo path verification). The `Project kind:
cargo` dispatch worked correctly; failure was upstream in the mise
install step, NOT in the new plain-cargo branch.

Likely fix: combine both rustup operations into a single
`powershell -NoProfile -File <script.ps1>` call (write a small .ps1
to the VM via vm push, then invoke once), OR semicolon-chain inside a
single PowerShell -Command instead of cmd-level `&&`.
