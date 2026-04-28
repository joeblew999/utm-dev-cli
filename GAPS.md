# Gap Analysis — utm-dev-cli

Punch list of what's missing, broken, or rough in the current codebase. Triaged by impact.

## Blocking / correctness

1. **`Commands::Icon` panics** — [src/cli.rs:71](src/cli.rs#L71) is `todo!("icon — generate platform icons")`. Advertised in `--help`; calling it crashes. Either implement (port from `utm-dev/.mise/tasks/icon`) or remove from the enum.

2. **Windows bootstrap doesn't install Rust** — [src/vm/bootstrap.rs](src/vm/bootstrap.rs) installs VS Build Tools, WebView2, mise — but never `mise use rust@stable`. Linux bootstrap does (line ~89). Windows builds work today only because the test repo's `mise.toml` declares `rust = "stable"` so `mise install` brings it in. A user project without that line silently fails. Fix: add `mise use --global rust@stable` after mise install on Windows.

3. **`disk_gib` is documented but unused on `vm up`** — [src/vm/profiles.rs:32](src/vm/profiles.rs#L32) docstring claims `vm up` auto-grows the qcow2 if profile size > current. The code in [src/cmd/vm.rs:263](src/cmd/vm.rs#L263) never reads `disk_gib`. `cargo check` already warns: `field disk_gib is never read`. Either wire it in or drop the field + docstring. Today users have to run `vm resize-disk` manually.

## Functional gaps (advertised but missing)

4. **`vm run`** — AGENTS.md describes it; not in `VmCommands` enum. Keep documented as future, but anyone reading the help is going to ask.

5. **Host setup on Windows** — [src/cmd/setup.rs:29](src/cmd/setup.rs#L29) bails with "setup not supported on this platform". Probably fine (utm-dev is Apple-Silicon-only by design) but the error message should say so explicitly: "utm-dev is macOS-only — UTM doesn't run elsewhere".

6. **`vm restart`** — small ergonomic gap. Today: `vm down && vm up`.

## Brittle / edge cases

7. **Linux bootstrap step 5 is dead code** — [src/vm/bootstrap.rs:96-105](src/vm/bootstrap.rs#L96-L105) gates `linux-dev` extras on `xdg-utils` not being installed, but step 2 already installs `xdg-utils` for **all** Linux profiles. The check always passes, step 5 never runs. Either swap the marker (e.g. `fonts-noto-color-emoji`) or move the GNOME-specific deps into step 2 with a `linux-dev`-only guard.

8. **CARGO_TARGET_DIR probe parsing is fragile** — [src/vm/build.rs:179-180](src/vm/build.rs#L179-L180) probes via `echo` and takes `lines().last()`. Works but assumes no trailing shell noise. A login banner or stray output breaks bundle resolution. Better: parse a fenced marker (e.g. `echo BEGIN; echo $CARGO_TARGET_DIR; echo END`).

9. **`vm package` hardcodes `joeblew999/`** — [src/cmd/vm.rs:591](src/cmd/vm.rs#L591) hint string. Cosmetic but ships a misleading suggestion to other users.

## Cleanup / dead code

10. **Eight `#[allow(dead_code)]` markers** — `profiles::DEFAULT_VM`, `profiles::vm_home`, `profiles::path_sep`, `state::clear`, `utm::find_vm_by_uuid`, `winrm::run_cmd`, `setup::_hush_unused`, `disk_gib`. Either use or delete. `_hush_unused` in particular smells like a leftover.

11. **`BootstrapMode::None` defined but no profile uses it** — [src/vm/profiles.rs:13](src/vm/profiles.rs#L13). Either add a profile that needs it or drop the variant.

## Tests

12. **No unit tests anywhere.** Pure-logic functions worth testing: `import::rewrite_plist_name`, `cmd::doctor::version_at_least`, `cmd::clean::find_target_dirs`, `winrm::extract_streams`, `winrm::extract_tag`. These don't need a VM and would catch regressions cheaply.

## Doc/UX

13. **`utm-dev init` writes Android-heavy `[tools]` block** — the demo doesn't need Java/Android. Consider splitting into `init` (minimal) + `init --android` (current behaviour).

14. **AGENTS.md "Future: vm run" mentions a Cloudflare logger** — pattern is valuable but undocumented. Worth a short ADR or design note before implementation drifts.

## Performance (low priority)

15. **VS Build Tools modify is 10–15 min** — running the full bootstrapper to add one component. A small win would be `vs_installer.exe modify --installPath X --add Y` directly, avoiding the bootstrapper download path. Not worth doing until it's actually painful; the current path is correct and runs at most once per VM.
