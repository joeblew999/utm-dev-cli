# Gap Analysis — utm-dev-cli

Punch list of what's missing, broken, or rough. Triaged by impact.

## Source-of-truth invariant

**Rust + tauri-cli + bun + node etc. are pinned by the user's project `mise.toml` — utm-dev's bootstrap does NOT install language runtimes.** The bootstrap only installs *non-mise-managed* prerequisites (apt deps, VS Build Tools, OpenSSH, WebView2). Anything mise can manage stays in mise's hands.

This is why a project consuming utm-dev MUST have `[tools] rust = "..."` (and `cargo:tauri-cli`, `bun`, etc.) declared in its `mise.toml` — `vm build` runs `mise install` inside the project dir to provision exactly those.

## Functional gaps (advertised but missing)

1. **`vm run`** — AGENTS.md describes it; not in `VmCommands` enum. Document as future, but anyone reading `--help` is going to ask.

2. **Linux x86_64 cross-compile** — `vm_build` rejects `--target x86-64` and `--target both` for Linux profiles. Real fix needs Debian multiarch (`dpkg --add-architecture amd64` + `:amd64` packages for libwebkit2gtk + friends + `gcc-x86-64-linux-gnu`). Plan it once Windows path is solid.

3. **`vm restart`** — small ergonomic gap. Today: `vm down && vm up`.

4. **`utm-dev setup` on Windows/Linux host bails** — fine because UTM only runs on macOS, but the message should say so explicitly: "utm-dev requires macOS — UTM doesn't run on Windows/Linux hosts".

5. **Linux bootstrap pre-installs Rust globally** — [src/vm/bootstrap.rs](src/vm/bootstrap.rs) step 4 runs `mise use --global rust@stable`. Per the source-of-truth invariant above, this is redundant: the project's mise.toml provisions Rust at build time. Remove it, then Linux + Windows bootstraps are symmetric.

## Brittle / edge cases

5a. **`winrm::run_elevated` polling can hang after the install completes** — the loop watches `(Get-ScheduledTask -TaskName 'BootstrapStep').State` for ≠ "Running" and `_ => {}` swallows WinRM errors. Observed on the ARM64 Windows VM: VS Build Tools install completed (`C:\vs-exit.txt` written with exit code 0) but the loop kept printing "still running" for 10+ minutes after, until manually killed. Fix: also poll for a sentinel file written *after* the install finishes (e.g. `C:\bootstrap-step-done.txt` written by the script after `Out-File 'C:\vs-exit.txt'`); when it exists, break regardless of what `Get-ScheduledTask` reports. Belt-and-braces over the existing state check.

6. **Linux bootstrap step 5 is dead code** — [src/vm/bootstrap.rs:96-105](src/vm/bootstrap.rs#L96-L105) gates `linux-dev` extras on `xdg-utils` not being installed, but step 2 already installs `xdg-utils` for **all** Linux profiles. The check always passes, step 5 never runs. Either swap the marker (e.g. `fonts-noto-color-emoji`) or merge into step 2 with a `linux-dev`-only guard.

7. **CARGO_TARGET_DIR probe parsing is fragile** — [src/vm/build.rs](src/vm/build.rs) probes via `echo` and takes `lines().last()`. A login banner or stray output breaks bundle resolution. Better: parse a fenced marker (e.g. `echo BEGIN; echo $CARGO_TARGET_DIR; echo END`).

8. **`vm package` hardcodes `joeblew999/`** — [src/cmd/vm.rs](src/cmd/vm.rs) hint string. Cosmetic but ships a misleading suggestion to other users.

## Tests

9. **No unit tests anywhere.** Pure-logic functions worth testing: `import::rewrite_plist_name`, `cmd::doctor::version_at_least`, `cmd::clean::find_target_dirs`, `winrm::extract_streams`, `winrm::extract_tag`. None of these need a VM.

## Doc/UX

10. **`utm-dev init` writes Android-heavy `[tools]` block** — the demo doesn't need Java/Android. Consider splitting into `init` (minimal) + `init --android` (current).

11. **AGENTS.md "Future: vm run" mentions a Cloudflare logger pattern** — pattern is valuable but undocumented. Worth a short ADR or design note before implementation drifts.

## Performance (low priority)

12. **VS Build Tools modify is 10–15 min** — running the full bootstrapper to add one component. A small win would be `vs_installer.exe modify --installPath X --add Y` directly. Not worth doing until painful; current path is correct and runs at most once per VM.

---

## Recently resolved

- **Cross-compile arm64+x86_64 on Windows** — `--target arm64|x86-64|both` shipped (commit f5460b6). Same VM produces both architectures via MSVC cross-tools.
- **VS Build Tools missed ARM64 component** — bootstrap now requires `VC.Tools.ARM64` and adds it to the install args (commit a686a24).
- **Dead code cleanup** — removed `_hush_unused`, `find_vm_by_uuid`, `vm_home`, `path_sep`, `DEFAULT_VM`, `winrm::run_cmd`, `BootstrapMode::None`, unused `disk_gib` field, and the panicking `Commands::Icon` stub.
