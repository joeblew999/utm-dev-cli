# Gap Analysis — utm-dev-cli

Punch list of what's missing, broken, or rough. Triaged by impact.

## Blocking / correctness

1. **Windows bootstrap doesn't pre-install Rust** — [src/vm/bootstrap.rs](src/vm/bootstrap.rs) installs VS Build Tools, WebView2, mise, but never `mise use rust@stable` (Linux bootstrap does at line ~89). Today, `vm build` works only because the user's project `mise.toml` declares `rust = "..."` so `mise install` brings it in. A user project without that line silently fails. Fix: add `mise use --global rust@stable` to the Windows bootstrap after mise is installed.

## Functional gaps (advertised but missing)

2. **`vm run`** — AGENTS.md describes it; not in `VmCommands` enum. Document as future, but anyone reading `--help` is going to ask.

3. **Linux x86_64 cross-compile** — `vm_build` rejects `--target x86-64` and `--target both` for Linux profiles. Real fix needs Debian multiarch (`dpkg --add-architecture amd64` + `:amd64` packages for libwebkit2gtk + friends + `gcc-x86-64-linux-gnu`). Plan it once Windows path is solid.

4. **`vm restart`** — small ergonomic gap. Today: `vm down && vm up`.

5. **`utm-dev setup` on Windows host bails** — fine because UTM only runs on macOS, but the message should say so explicitly: "utm-dev requires macOS — UTM doesn't run on Windows/Linux hosts".

## Brittle / edge cases

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
