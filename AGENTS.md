# Agent Guidelines: utm-dev-cli

Rust CLI rewrite of [utm-dev](https://github.com/joeblew999/utm-dev) TypeScript tasks.
Binary name: `utm-dev`. Published to crates.io so consuming repos install via `cargo:utm-dev`.

User-facing docs live in [README.md](./README.md). This file is for AI assistants and contributors — it captures *non-obvious* invariants that aren't visible from reading the code.

## Golden rule: dovetail with UTM, don't fight it

UTM owns display, drivers, storage, and hardware config. `utm-dev` only orchestrates **lifecycle** (start/stop), **networking** (port forwards via AppleScript), and **remote execution** (SSH + WinRM). Never try to fix display issues, install guest drivers, or reconfigure hardware from code — that's UTM's job.

## Use mise tasks, not raw cargo

This repo uses `mise.toml` as the single source of truth for build/install/test:

```sh
mise run build      # cargo build
mise run install    # cargo install --path .
mise run test       # cargo test
```

Don't call cargo directly when a task exists. Add a task to `mise.toml` if one is missing.

## Source-of-truth invariant for VM bootstrap

Bootstrap installs **only** non-mise-managed prerequisites — apt deps, VS Build Tools + WebView2, OpenSSH, sshd config, host pubkey. **It does NOT install Rust, tauri-cli, bun, node, or anything mise can manage.** Those come from the user's project `mise.toml` when `vm build` runs `mise install` inside the project dir.

Consequence: a project consuming utm-dev MUST declare its language runtimes in `mise.toml` (e.g. `[tools] rust = "stable"`, `"cargo:tauri-cli" = "2"`). If a runtime isn't there, the build fails — and that's the *correct* failure mode, because the project should pin its toolchain for reproducibility.

## Test repo for end-to-end

A real Tauri starter at `~/workspace/go/src/github.com/joeblew999/utm-dev-demo` is the canonical fixture for validating `vm build` changes. Don't scaffold throwaway test projects.

## Box source

Boxes come from the **`utm` Vagrant Cloud registry** — pre-built UTM VMs with VirtIO drivers, WinRM (Windows), and SSH already configured:

```
https://app.vagrantup.com/utm/{box_name}                  (browse)
{API}/box/{box_name}/versions                             (latest version)
{API}/box/{box_name}/version/{ver}/provider/utm/architecture/arm64/download
```

(`{API}` = `https://api.cloud.hashicorp.com/vagrant/2022-09-30/registry/utm`)

Box names: `windows-11`, `ubuntu-24.04`, `debian-12`. Each is a `.tar.gz` (renamed `.box`) wrapping a `.utm` bundle. Cached at `~/.cache/utm-dev/{box}_{version}_arm64.box`.

## Imported-bundle quirk: rewrite plist Name before import

UTM imports use the bundle's `config.plist` `Name` field as the on-disk display name. Two profiles using the same box (e.g. `linux-test` + `linux-build` both on `ubuntu-24.04`) collide in `~/Library/Containers/com.utmapp.UTM/Data/Documents/`. `import.rs` copies the cached bundle, rewrites the plist Name to the **profile name**, then imports — so each profile lands as its own `.utm` bundle. After import, snapshot UTM's UUID list before/after to detect which UUID was just created (UTM doesn't return it directly).

## State: profile name vs UTM display name vs UUID

`.mise/state/vm-{profile}.json` stores the actual UTM `display_name` and `uuid`. Always use `state.display_name` (not `profile.box_name`) for UTM operations after import. UTM may rename bundles internally; the state file is the source of truth.

## SSH auth order

1. SSH agent (macOS Keychain / ssh-agent)
2. Key files: `~/.ssh/id_ed25519`, `~/.ssh/id_rsa`, `~/.ssh/id_ecdsa`
3. Password from profile

Bootstrap installs the host's pubkey into both Linux (`~/.ssh/authorized_keys`) and Windows (`~/.ssh/authorized_keys` **and** `C:\ProgramData\ssh\administrators_authorized_keys` — Windows OpenSSH's `Match Group administrators` redirects admin users to the latter).

## Streaming exec on Linux needs `-tt`

`ssh::exec_streaming` injects `-tt` only on Linux — it forces a pseudo-TTY so cargo/mise stay line-buffered (without it, long compiles look frozen for 10+ min). Windows cmd.exe **breaks** with `-tt` (the session exits immediately returning 0), so Windows uses plain pipes and we redirect to a log file at the cmd level for visibility (`vm logs --name X --follow`).

## Windows bootstrap landmines

- VS Build Tools `--includeRecommended` does **not** install the native ARM64 compiler. Must explicitly `--add Microsoft.VisualStudio.Component.VC.Tools.ARM64`. The vswhere check should require that component, not `VC.Tools.x86.x64`, otherwise old broken installs get skipped as "already installed" and the build then fails with no compiler for the host arch.
- `vs_buildtools.exe` with `--add` modifies an existing install in place — so the same code path covers fresh-install and migration.
- `LocalAccountTokenFilterPolicy = 1` is mandatory for WinRM with local admin accounts.
- Long-running PS scripts go through `winrm::run_elevated` which writes to `C:\bootstrap-step.ps1` and runs as SYSTEM via a scheduled task — bypasses UAC and survives WinRM dropouts during heavy I/O.

## Windows cmd.exe gotchas in build.rs

- `if not exist X CMD1 && CMD2` parses as `if not exist X (CMD1 && CMD2)` — on a re-run where X exists, **none** of the chain runs. Use unconditional `&` plus `mkdir 2>nul` to swallow the "exists" error.
- libssh2 SCP doesn't translate `C:/...` paths into anything OpenSSH-SCP accepts. Use **relative** remote paths on Windows (lands in user's home directory); use absolute Unix paths on Linux.

## AppleDouble files break Tauri

`tar` on macOS emits `._*` HFS metadata stubs that aren't valid UTF-8. Tauri's build script reads everything in `src-tauri/capabilities/` and crashes on them. `build.rs` excludes `._*` and sets `COPYFILE_DISABLE=1` when archiving.

## vm run + observability

`vm run --name X --bin <path>` launches a built binary inside the VM and captures stdout/stderr to `~/.utm-dev-run/run.log` (Linux: via `xvfb-run`; Windows: via `Start-Process` detached). Tail with `vm logs --kind run --follow`.

For richer observability, apps can embed a Cloudflare-Worker-streaming logger that publishes startup events out-of-band. utm-dev stays unaware of that logger — it's the app's concern. See [`docs/adr-001-vm-run-observability.md`](docs/adr-001-vm-run-observability.md) for the rationale and integration sketch.
