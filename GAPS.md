# Gap Analysis — utm-dev-cli

Punch list of what's missing or rough. Triaged by impact.

## Source-of-truth invariant

**Rust + tauri-cli + bun + node etc. are pinned by the user's project `mise.toml` — utm-dev's bootstrap does NOT install language runtimes.** The bootstrap only installs *non-mise-managed* prerequisites (apt deps, VS Build Tools, OpenSSH, WebView2, Defender exclusions). Anything mise can manage stays in mise's hands.

This is why a project consuming utm-dev MUST have `[tools] rust = "..."` (and `cargo:tauri-cli`, `bun`, etc.) declared in its `mise.toml` — every build runs `mise install` (in the VM for cross-target, on the host for `mac build`) to provision exactly those.

## Validated end-to-end

The full pipeline is **proven against utm-dev-cli itself** (plain-cargo) AND **utm-dev-demo** (Tauri starter):

| Phase | Mac native (host) | Linux ARM64 | Windows x86_64 |
|---|---|---|---|
| `<platform> build` first run | seconds (cargo cache) | ~14 min | ~18 min cargo + ~40 sec mise install |
| `<platform> build` cached re-run | sub-second | unmeasured | ~3 min |
| Output dir | `.build/macos/<arch>/<bin>` | `.build/linux/<arch>/<bin or bundle>` | `.build/windows/<arch>/<bin or .exe>` |
| Runs natively | ✓ verified `--version` and `--help` | n/a | ✓ verified `--version` on the Win11 VM |

`utm-dev mac build`, `utm-dev linux build`, `utm-dev windows build` all share one surface. All three honour the project's `mise.toml`. All three handle Tauri AND plain cargo via `ProjectKind` detection.

## Functional gaps

1. **Windows ARM64 native build — BLOCKED on Microsoft.**
   VS Build Tools on ARM64 hosts ships `Hostarm64\x64` and `Hostarm64\x86` cross-tools but no `Hostarm64\arm64` (native ARM64 toolchain). `vs_buildtools.exe --add Microsoft.VisualStudio.Component.VC.Tools.ARM64` and `vs_installer.exe modify --add ...` both return exit 0 without installing the component.
   **Workaround in place:** cross-compile x86_64 from ARM64 (Hostarm64\x64), runs under Windows ARM64 emulation. x86_64 is what most Windows users actually ship anyway. Re-test periodically as MSVC catches up.

2. **Linux x86_64 cross-compile — partially done.**
   Linux ARM64 → ARM64 native works. Linux ARM64 → x86_64 needs Debian multiarch (libwebkit2gtk-4.1-dev:amd64 + gcc-x86-64-linux-gnu + linker env). `ensure_linux_multiarch` provisions on first `--target x86-64` invocation. Tested ad-hoc, not recently dogfooded.

3. **Tauri Windows release builds exit silently in headless `vm run`.**
   Tauri Windows release apps use the GUI subsystem (no stdout) and exit immediately when launched in a non-interactive desktop session. **Not a utm-dev bug** — Win32 GUI subsystem + headless SSH session interaction. For visual verification: RDP into the VM (port `3389` forwarded), or have the app embed an out-of-band logger.

4. **`vm screenshot` against Tauri Linux returns a black PNG.**
   Bare Xvfb has no GL backend. WebKit-GTK content (the WebView Tauri renders into) requires GL/EGL and silently doesn't paint. Process-level verification (`pgrep`, run.log) works fine.

5. **"Last error stanzas" filter is a red-herring on build failure.**
   The host's error filter in `vm/build.rs` greps the entire build log for any "error"-shaped line and shows them after a build fails. It frequently surfaces unrelated PowerShell parser errors from earlier in the log, sending the next debugger chasing ghosts. Cost us ~30 min during the 2026-05-03 dogfood. **Fix: only show error stanzas after the LAST `Compiling` line, or inside the last failure block.** ~30 line change.

6. **`vm package` exports as Vagrant `.box` — not yet tested for Apple Silicon redistribution.**
   Code path exists; never validated by another machine importing the produced `.box`.

7. ~~Missing: `utm-dev mcp` subcommand~~ — **DONE**. Ported from
   `joeblew999/utm-dev/.mise/tasks/mcp.ts` to `src/cmd/mcp.rs`. Generates
   `.mcp.json` + `.claude/settings.json` (context7 + mise MCP servers,
   auto-allow permissions). Tested end-to-end in tmpdir — first run
   creates both files with absolute resolved bin paths, second run is a
   clean no-op.

8. ~~WebDriver-based screenshot~~ — **PORTED, NEEDS REAL TAURI TEST**.
   `utm-dev screenshot` ported from `screenshot.ts` + `_screenshot.ts` to
   `src/cmd/screenshot.rs`. Walks up from cwd to find `src-tauri/`, builds
   with `--features webdriver`, spawns `tauri-webdriver` proxy + the app,
   creates a W3C WebDriver session, captures via
   `GET /session/<id>/screenshot`. Cleans up procs + `_si.sock` files on
   Drop. Compiles cleanly; **end-to-end testing requires a real Tauri
   project that exposes the `webdriver` cargo feature + `tauri-webdriver`
   on PATH** (`cargo install tauri-webdriver --locked`). Until validated
   against e.g. utm-dev-demo, treat as code-complete-but-unproven.

## Future direction

**ewe-studios/ewe_platform/foundation_testbed** — a Linux-host equivalent of utm-dev. Same problem space, different host OS. Worth understanding for cross-pollination: shared CLI surface? shared mise-task layer? unified harness across Mac+Linux hosts?

**`joeblew999/utm-dev` (TypeScript) — superseded; safe to archive after WebDriver screenshot is validated.**
Last commit 2026-03-25, tagged v2.1.0. 18 mise tasks in TypeScript. utm-dev-cli is now a full superset: gap #7 (mcp) is done, gap #8 (WebDriver screenshot) is ported but unproven against a real Tauri project. Once #8 is validated, tag the TS repo final and archive; consumer Tauri repos that pin `git::utm-dev//.mise/tasks?ref=v2.1.0` should migrate to invoking `utm-dev-cli` directly.

(Note: `docs/web/` in the utm-dev repo is unrelated CAD-app content — copy-paste mishap from another project. Not utm-dev documentation, ignore it.)

**Expose utm-dev as an MCP server via [turbomcp](https://github.com/Epistates/turbomcp).** The CLI surface (vm up/down/build/exec/logs/clean/debloat/...) maps cleanly onto MCP tools. Devs and AI assistants would then drive cross-platform builds via standard MCP tooling instead of shelling out. CLI stays the engine; MCP is a thin adapter on top.

**Dogfood Tauri:** [turbomcpstudio](https://github.com/Epistates/turbomcpstudio) (Tauri wrapping turbomcp) — its first successful Windows + Linux build is the readiness milestone before MCP work starts.

---

## Recently resolved

### 2026-05-03 — infrastructure pass (commit `4a43ce2`)
- **Isomorphic mac/linux/windows build harness.** Same flags, same `.build/<platform>/<arch>/` output dir, same mise.toml-respecting cargo invocation. Native mac builds use `mise exec -- cargo` when possible.
- **`vm clean`** with default / `--deep` (cargo target + mise installs) / `--aggressive` (one-shot Windows tweaks: powercfg /h off, compact /CompactOS, vssadmin delete shadows, pagefile to D:, wevtutil cl). Live freed 4.8 GB.
- **`vm debloat`** — Windows Store-app removal, safelist cross-checked against Raphire/Win11Debloat + Sycnex/Windows10Debloater + ChrisTitusTech/winutil. Skips system-pinned packages.
- **Defender exclusions auto-applied at bootstrap** for cargo / rustup / mise / D:\target / .utm-dev-build. Was the root cause of the "process cannot access the file" failures during mise install / cargo build.
- **Drop `ssh2` (libssh2 + vendored OpenSSL chain).** ssh module rewritten as `ssh` / `scp` CLI subprocess transport. No more perl-on-VM build dep, no openssl-sys, no native build deps. `Session` is now a cheap profile-bearing handle (no persistent TCP).
- **Drop `reqwest` (tokio + hyper chain).** ureq 3 + rustls + platform-verifier across import.rs / winrm.rs / utm.rs. Native cert stores via platform-verifier. ~50 transitive deps gone, binary 4.2 MB (was ~5-6 MB).
- **`cmd/vm.rs` split** 1530 → 584 lines. Subcommands extracted to `src/cmd/vm/{clean,debloat,doctor,run,resize_disk,package}.rs`.
- **scp pull uses forward slashes for Windows remote paths** through the ssh CLI (backslashes get eaten by shell expansion).
- **ssh transport `from_utf8_lossy` on raw bytes** instead of strict `read_to_string`. Windows tools (DISM, Get-Content, mise console) emit local codepage / UTF-16 with BOMs — strict UTF-8 was bailing with empty output.
- **PowerShell `switch_rustup` quoting bug** — combined into a single `powershell -Command "..."` call instead of two `&&`-chained PS calls.
- **Plain-cargo path verified end-to-end.** `utm-dev windows build --release` against utm-dev-cli (no `src-tauri/`) → 4.4 MB PE32+ x86_64 PE binary that executes correctly on the Win11 VM.

### Earlier
- **End-to-end validation against utm-dev-demo** — Windows .msi/.exe + Linux .deb/.rpm/.AppImage all produced and pulled to host.
- **Pre-baked box pipeline** — `download_prebaked` resume-aware fetcher. Onboarding from ~30 min bootstrap to ~5 min download.
- **`MISE_CARGO_BINSTALL=true`** + cargo-binstall pre-installed (Linux + Windows) — drops per-project tauri-cli install from 25 min → 30 sec.
- **sccache wrapping** via RUSTC_WRAPPER + `cargo:sccache` from mise.
- **GitHub Actions release workflow** — tag `vX.Y.Z` produces a mac-arm64 binary; install via `mise use ubi:joeblew999/utm-dev-cli@latest`.
- **`vm build` mise.toml pre-flight** — bails in 50 ms if `rust + cargo:tauri-cli` aren't pinned.
- **Per-phase elapsed timing** in `vm build`.
- **Cross-compile x86_64 on Windows from ARM64 VM** — `--target x86-64` works.
- **`winrm::run_elevated` polling hang** — sentinel-file completion detection.
- **CARGO_TARGET_DIR probe** — fenced `BEGIN_CTD/END_CTD` markers.
- **WebView2 install via Evergreen Bootstrapper** — winget unreliable on fresh Vagrant boxes.
- **`cd /d` for Windows bundle archive** — bare `cd` doesn't switch drives in cmd.exe.
- **vm run process detachment on Linux** — bypassed libssh2 + `-tt`; use direct ssh subprocess + setsid -f.
- **Windows vm run PowerShell single-line** — cmd's `^<nl>` continuation doesn't survive SSH delivery.
