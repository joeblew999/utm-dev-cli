# Gap Analysis — utm-dev-cli

Punch list of what's missing or rough. Triaged by impact.

## Source-of-truth invariant

**Rust + tauri-cli + bun + node etc. are pinned by the user's project `mise.toml` — utm-dev's bootstrap does NOT install language runtimes.** The bootstrap only installs *non-mise-managed* prerequisites (apt deps, VS Build Tools, OpenSSH, WebView2). Anything mise can manage stays in mise's hands.

This is why a project consuming utm-dev MUST have `[tools] rust = "..."` (and `cargo:tauri-cli`, `bun`, etc.) declared in its `mise.toml` — `vm build` runs `mise install` inside the project dir to provision exactly those.

## Functional gaps

1. **Windows ARM64 native build** — current VS Build Tools install on ARM64 hosts ships `Hostarm64\x64` and `Hostarm64\x86` cross-tools but no `Hostarm64\arm64` (native ARM64 toolchain). Both `vs_buildtools.exe --add Microsoft.VisualStudio.Component.VC.Tools.ARM64` and `vs_installer.exe modify --add ...` returned exit code 0 without actually installing the component — looks like Microsoft's installer doesn't yet ship a native ARM64-host-targeting-ARM64 MSVC toolchain. We work around by cross-compiling x86_64 from ARM64 (runs under Windows ARM64 emulation). Re-test periodically as MSVC catches up.

2. **`vm run`** — AGENTS.md describes it; not in `VmCommands` enum. Document as future, but anyone reading `--help` is going to ask.

3. **Linux x86_64 cross-compile** — `vm_build` rejects `--target x86-64` and `--target both` for Linux profiles. Real fix needs Debian multiarch (`dpkg --add-architecture amd64` + `:amd64` packages for libwebkit2gtk + friends + `gcc-x86-64-linux-gnu`). Plan it once Windows path is solid.

## Tests

4. **No unit tests anywhere.** Pure-logic functions worth testing: `import::rewrite_plist_name`, `cmd::doctor::version_at_least`, `cmd::clean::find_target_dirs`, `winrm::extract_streams`, `winrm::extract_tag`. None of these need a VM.

## Doc/UX

5. **`utm-dev init` writes Android-heavy `[tools]` block** — the demo doesn't need Java/Android. Consider splitting into `init` (minimal) + `init --android` (current).

6. **AGENTS.md "Future: vm run" mentions a Cloudflare logger pattern** — pattern is valuable but undocumented. Worth a short ADR or design note before implementation drifts.

## Future direction

**Expose utm-dev as an MCP server via [turbomcp](https://github.com/Epistates/turbomcp).** The CLI surface (vm up/down/build/exec/logs/...) maps cleanly onto MCP tools. Devs and AI assistants would then drive cross-platform Tauri builds via standard MCP tooling instead of shelling out. Keep CLI as the underlying engine; MCP is a thin adapter on top.

**Dogfood loop:** [turbomcpstudio](https://github.com/Epistates/turbomcpstudio) is itself a Tauri app that wraps turbomcp — so we build it (cross-platform) *with utm-dev*, in order to ship the GUI that talks to the MCP server we'll later expose *from* utm-dev. utm-dev has to be reliable enough to build a non-trivial Tauri app (turbomcpstudio is a real validator, not a vanilla starter). Treat its first successful Windows + Linux build as the readiness milestone before MCP work starts.

## Performance (low priority)

7. **VS Build Tools modify is 10–15 min** — running the full bootstrapper to add one component. A small win would be `vs_installer.exe modify --installPath X --add Y` directly. Not worth doing until painful; current path is correct and runs at most once per VM.

---

## Recently resolved

- **Cross-compile arm64+x86_64 on Windows** — `--target arm64|x86-64|both` shipped (commit f5460b6), then pivoted to x86_64-only on Windows due to MSVC ARM64 toolchain gap (commit 1954cde).
- **VS Build Tools missed ARM64 component** — bootstrap now requires `VC.Tools.ARM64` and adds it to the install args (commit a686a24). Component still doesn't actually install — see gap #1.
- **`winrm::run_elevated` polling hang** — sentinel-file completion detection added (commit b742922).
- **Two-phase mise install on Windows ARM64** — `mise install rust` first, then switch rustup default-host to x86_64, then `mise install` (rest). Necessary because MSVC has no ARM64 native linker on ARM64 hosts.
- **Linux bootstrap step 5 was dead code** — marker now checks `fonts-noto-color-emoji` (only step-5-specific), not `xdg-utils` (which step 2 already installs).
- **CARGO_TARGET_DIR probe robustness** — fenced `BEGIN_CTD/END_CTD` markers so stray output doesn't corrupt bundle path resolution.
- **`vm restart`** — added.
- **`utm-dev setup` error message** — explicit about utm-dev being macOS-only for VM orchestration.
- **`vm package` no longer hardcodes `joeblew999/`** — generic `<username>/` placeholder.
- **Linux bootstrap pre-installing Rust globally** — removed (per source-of-truth invariant).
- **Dead code cleanup** — removed `_hush_unused`, `find_vm_by_uuid`, `vm_home`, `path_sep`, `DEFAULT_VM`, `winrm::run_cmd`, `BootstrapMode::None`, unused `disk_gib` field, and the panicking `Commands::Icon` stub.
