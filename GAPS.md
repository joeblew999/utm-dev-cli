# Gap Analysis — utm-dev-cli

Punch list of what's missing or rough. Triaged by impact.

## Source-of-truth invariant

**Rust + tauri-cli + bun + node etc. are pinned by the user's project `mise.toml` — utm-dev's bootstrap does NOT install language runtimes.** The bootstrap only installs *non-mise-managed* prerequisites (apt deps, VS Build Tools, OpenSSH, WebView2). Anything mise can manage stays in mise's hands.

This is why a project consuming utm-dev MUST have `[tools] rust = "..."` (and `cargo:tauri-cli`, `bun`, etc.) declared in its `mise.toml` — `vm build` runs `mise install` inside the project dir to provision exactly those.

## Functional gaps

1. **Windows ARM64 native build — BLOCKED on MS toolchain.**
   VS Build Tools on ARM64 hosts ships `Hostarm64\x64` and `Hostarm64\x86` cross-tools but no `Hostarm64\arm64` (native ARM64 toolchain).
   - `vs_buildtools.exe --add Microsoft.VisualStudio.Component.VC.Tools.ARM64 --quiet --norestart --wait` returned exit 0 without installing the component.
   - `vs_installer.exe modify --add Microsoft.VisualStudio.Component.VC.Tools.ARM64 --quiet --norestart --wait` likewise: exit 0, nothing installed.
   - Microsoft appears to not yet ship a native ARM64-host-targeting-ARM64 MSVC toolchain. Possibly the component name has changed, or it requires a different invocation path.
   **Workaround in place:** cross-compile x86_64 from ARM64 (Hostarm64\x64), runs under Windows ARM64 emulation. x86_64 is what most Windows users actually ship anyway. Re-test periodically as MSVC catches up.

2. **`vm run` — SCAFFOLD LANDED, needs validation.**
   `utm-dev vm run --name <vm> --bin <vm-side-path>` ships in this commit. Linux uses `xvfb-run -a` so GUI apps boot headlessly; Windows uses PowerShell `Start-Process` detached. Output goes to `~/.utm-dev-run/run.log` (Linux) or `%USERPROFILE%\.utm-dev-run\run.log` (Windows); tail it with `vm logs --kind run --follow`. Auto-detection of the binary path from the latest `.build/<platform>/<arch>/` is a follow-up.

3. **Linux x86_64 cross-compile — LANDED, needs validation.**
   Multiarch system libs install on demand inside `vm build` when the requested target is `x86-64` or `both` on a Linux profile (`build::ensure_linux_multiarch`). Adds `gcc-x86-64-linux-gnu` and `:amd64` versions of libwebkit2gtk-4.1-dev / libgtk-3-dev / libayatana-appindicator3-dev / librsvg2-dev / libssl-dev / libxdo-dev / libsoup-3.0-dev / libjavascriptcoregtk-4.1-dev. `cargo tauri build --target x86_64-unknown-linux-gnu` then runs with `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc` + multiarch `PKG_CONFIG_PATH`. ~500 MB–1 GB of disk on first run; subsequent runs no-op.

## Doc/UX

4. **AGENTS.md "Future: vm run" mentions a Cloudflare logger pattern** — pattern is valuable but undocumented. Worth a short ADR or design note before implementation drifts. (Mobile: iOS/Android can be done later, not blocking Windows/Linux desktop work.)

## Future direction

**Expose utm-dev as an MCP server via [turbomcp](https://github.com/Epistates/turbomcp).** The CLI surface (vm up/down/build/exec/logs/...) maps cleanly onto MCP tools. Devs and AI assistants would then drive cross-platform Tauri builds via standard MCP tooling instead of shelling out. Keep CLI as the underlying engine; MCP is a thin adapter on top.

**Dogfood loop:** [turbomcpstudio](https://github.com/Epistates/turbomcpstudio) is itself a Tauri app that wraps turbomcp — so we build it (cross-platform) *with utm-dev*, in order to ship the GUI that talks to the MCP server we'll later expose *from* utm-dev. utm-dev has to be reliable enough to build a non-trivial Tauri app (turbomcpstudio is a real validator, not a vanilla starter). Treat its first successful Windows + Linux build as the readiness milestone before MCP work starts.

## Performance (low priority)

5. **VS Build Tools modify is 10–15 min** — running the full bootstrapper to add one component. A small win would be `vs_installer.exe modify --installPath X --add Y` directly. Not worth doing until painful; current path is correct and runs at most once per VM.

---

## Recently resolved

- **Pre-baked box pipeline** — `prebaked_url: Option<&str>` on VmProfile + `download_prebaked` resume-aware fetcher. Onboarding goes from ~30 min bootstrap to ~5 min download. See [docs/box-publishing.md](docs/box-publishing.md).
- **`MISE_CARGO_BINSTALL=true`** — drops the per-project tauri-cli install from a 25-min compile to a ~30-sec download via cargo-binstall (mise auto-bootstraps it).
- **`vm doctor`** — Linux + Windows health checks (mise, build tools, cross-toolchain, WebView2, rustup default-host). Emits ⚠ for known-blocked items (BLOCKED_BY_MS) and exits 0 if only those fail; ✗ + exit 1 for actionable failures.
- **`vm logs --tail N` / `--errors`** — fast error discovery on failed builds.
- **Auto-error-dump on `vm build` failure** — bail prints the error stanza tail before the exit; no need to run `vm logs` after.
- **`vm build` mise.toml pre-flight** — bails in 50 ms if rust + cargo:tauri-cli aren't pinned, instead of 25 min into a doomed VM run.
- **Per-phase elapsed timing in `vm build`** — `⌚ sync: 3s | mise install: 8m | cargo tauri build: 22m | total: 30m`.
- **GitHub Actions release workflow** — tag `vX.Y.Z` produces a mac-arm64 binary attached to the release; devs install via `mise use ubi:joeblew999/utm-dev-cli@latest`.
- **Cross-compile x86_64 on Windows from ARM64 VM** — `--target x86-64` ships clean (commits 1954cde + 92b3f3c). Windows arm64 native blocked by MS — see #1.
- **Linux x86_64 cross-compile** — Debian multiarch + gcc-x86-64-linux-gnu + linker env, on demand.
- **`vm run` scaffold** — Linux xvfb-run, Windows Start-Process detached, output captured to `~/.utm-dev-run/run.log`.
- **`vm restart`, `vm package` hint, setup error message** — small ergonomic fixes.
- **`utm-dev init` Android-heavy default** — split into `init` (minimal) and `init --android` (full block).
- **`winrm::run_elevated` polling hang** — sentinel-file completion detection added.
- **CARGO_TARGET_DIR probe** — fenced `BEGIN_CTD/END_CTD` markers.
- **WebView2 install via Evergreen Bootstrapper** — winget Microsoft.EdgeWebView2Runtime is unreliable on fresh Vagrant boxes; switched to MS's documented headless installer.
- **Dead code cleanup** — removed `_hush_unused`, `find_vm_by_uuid`, `vm_home`, `path_sep`, `DEFAULT_VM`, `winrm::run_cmd`, `BootstrapMode::None`, unused `disk_gib`, panicking `Commands::Icon` stub.
- **Linux bootstrap pre-installing Rust globally** — removed (per source-of-truth invariant). Step 5 dead-code marker swapped from `xdg-utils` to `fonts-noto-color-emoji`.
