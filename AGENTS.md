# Agent Guidelines: utm-dev-cli

Rust CLI rewrite of [utm-dev](https://github.com/joeblew999/utm-dev) TypeScript tasks.
Binary name: `utm-dev`. Published to crates.io so consuming repos install via `cargo:utm-dev`.

## Golden rule: dovetail with UTM, don't fight it

UTM manages VM display, drivers, storage, and hardware configuration. `utm-dev` only
orchestrates **lifecycle** (start/stop), **networking** (port forwards via AppleScript),
and **remote execution** (SSH). Never try to fix display issues, install guest OS drivers,
or reconfigure hardware from code — those are UTM's job.

## Box source

Boxes come from the **`utm` Vagrant Cloud registry** — pre-built UTM VMs with VirtIO
drivers, WinRM (Windows), and SSH already configured:

```
https://app.vagrantup.com/utm/{box_name}                  (browse)
https://api.cloud.hashicorp.com/vagrant/2022-09-30/registry/utm/box/{box_name}/versions  (latest version)
https://api.cloud.hashicorp.com/vagrant/2022-09-30/registry/utm/box/{box_name}/version/{ver}/provider/utm/architecture/arm64/download  (download URL)
```

Box names: `windows-11`, `ubuntu-24.04`, `debian-12`
Box format: `.tar.gz` (a renamed `.box`) containing a `.utm` bundle directory.
Cache: `~/.cache/utm-dev/{box_name}_{version}_arm64.box`

## UTM AppleScript: correct import call

```applescript
tell application "UTM" to import new virtual machine from POSIX file "/path/to/vm.utm"
```

NOT `open POSIX file` — that does something different. After import, snapshot UUIDs
before/after to detect which UUID was just created (UTM doesn't return it directly).

## Architecture

```
src/
  main.rs             entry point
  cli.rs              clap Commands enum
  cmd/
    doctor.rs   ✓     checks tool availability via which()
    platform.rs       stubs — mac/ios/android/windows/linux/all
    vm.rs       ✓     up / down / exec / adopt / ls implemented
  vm/
    mod.rs      ✓
    profiles.rs ✓     5 static profiles (box names, ports, credentials)
    state.rs    ✓     .mise/state/vm-{name}.json — uuid + display_name
    utm.rs      ✓     ensure_utm, list_vms, start_vm, stop_vm, wait_for_boot,
                      configure_network (AppleScript), configure_resources
    ssh.rs      ✓     connect (agent → key files → password), exec, exec_with_exit,
                      upload (SCP), check
    import.rs   ✓     Vagrant Cloud API download, extract, import via AppleScript
    bootstrap.rs ✓    Linux SSH bootstrap (apt + mise + Rust); Windows stub
```

## VM profiles

| Profile | Box | OS | SSH | WinRM | RAM |
|---|---|---|---|---|---|
| windows-build | windows-11 | Windows ARM64 | 2222 | 5985 | 12288 MiB |
| windows-test  | windows-11 | Windows ARM64 | 2322 | 6985 | 4096 MiB  |
| linux-build   | ubuntu-24.04 | Linux ARM64 | 2422 | — | 4096 MiB |
| linux-test    | ubuntu-24.04 | Linux ARM64 | 2522 | — | 2048 MiB |
| linux-dev     | debian-12  | Linux ARM64 (GNOME) | 2622 | — | 6144 MiB |

Credentials: `vagrant` / `vagrant` for all. Boxes pre-configure WinRM on Windows.

## vm adopt — for existing non-Vagrant VMs

If the user already has a UTM VM (e.g. `plat-windows`) that isn't from the Vagrant registry:

```bash
utm-dev vm adopt --name windows-build --utm-name plat-windows
```

This writes `.mise/state/vm-windows-build.json` and skips the download/import step.
The user must have set up the VM in UTM themselves (VirtIO drivers, SSH, port forwards).

**Black screen on non-Vagrant Windows VMs**: means VirtIO GPU driver is missing.
Fix via UTM's GUI (attach VirtIO driver ISO from VM settings → Drives) — do NOT try
to fix this from code.

## vm up flow

```
ensure_utm()
  → if no state: import::ensure_imported()  ← Vagrant Cloud download + AppleScript import
  → configure_network() + configure_resources()
  → start_vm(display_name)           ← uses state.display_name, NOT profile.box_name
  → wait_for_boot()                  ← WinRM (Windows) or SSH (Linux)
  → bootstrap::run()                 ← Linux: apt + mise + Rust; Windows: OpenSSH via WinRM
  → state::save()
```

## SSH auth order

1. SSH agent (macOS Keychain / ssh-agent)
2. Key files: `~/.ssh/id_ed25519`, `~/.ssh/id_rsa`, `~/.ssh/id_ecdsa`
3. Password from profile (fallback)

Windows VMs from the Vagrant registry have SSH pre-configured with the `vagrant` password.

## Conventions

- Return `anyhow::Result<()>` from all commands
- Progress: `println!("→ ...")`, success: `println!("✓ ...")`, fatal: `anyhow::bail!(...)`
- Unimplemented: `todo!("clear message")`
- Never `unwrap()` in user-facing paths
- State files use actual UTM display names — `state.display_name` may differ from `profile.box_name`

## Implementation status

| Command | Status |
|---|---|
| `doctor` | ✓ |
| `vm ls` | ✓ |
| `vm adopt` | ✓ |
| `vm up` | ✓ (import + bootstrap + idempotent re-runs) |
| `vm down` | ✓ |
| `vm exec` | ✓ |
| `vm build` | ✓ (sync → build → pull artifacts) |
| `vm delete` | ✓ (utmctl + AppleScript fallback) |
| `vm package` | ✓ (export as Vagrant .box) |
| `setup` / `init` | stub |

## vm build flow

```
ssh::connect()            ← auto-starts VM if not reachable
tar (exclude target/.git/node_modules) → SCP upload → untar on VM
mise trust && mise install            ← idempotent tool install
mise run build                        ← Tauri build
tar artifacts on VM → SCP download → extract to .build/{platform}/
```

Artifacts land in `<project>/.build/windows/*.{msi,exe}` or `.build/linux/*.{deb,AppImage,rpm}`.

## WinRM bootstrap flow

Implemented in `src/vm/winrm.rs` (pure Rust SOAP client, no pywinrm).
Called from `bootstrap::run` for Windows VMs.

```
winrm.ping()                                    ← check WinRM reachable
Add-WindowsCapability OpenSSH.Server (elevated) ← SYSTEM scheduled task
Start-Service sshd / Set-Service Automatic
Set administrators_authorized_keys              ← host public key
Write minimal sshd_config
LocalAccountTokenFilterPolicy = 1
irm https://mise.run/install.ps1 | iex          ← mise
```

## Linux bootstrap (idempotent)

```
dpkg -s build-essential    → install if missing
dpkg -s libwebkit2gtk-4.1-dev → Tauri deps if missing  
mise --version             → install if missing
rustc --version            → mise use rust@stable if missing
linux-dev: xdg-utils + GNOME check
```

Both Linux and Windows bootstraps install the host's SSH public key into
the VM (Linux: `~/.ssh/authorized_keys`; Windows: BOTH that path AND
`C:\ProgramData\ssh\administrators_authorized_keys` because vagrant is an
admin and Windows OpenSSH's `Match Group administrators` redirects admin
users to the latter). Result: passwordless `ssh`, `scp`, and VS Code Remote
SSH against the VMs out of the box.

## Demo repo for end-to-end testing

A real Tauri starter lives at:

```
~/workspace/go/src/github.com/joeblew999/utm-dev-demo
```

Scaffolded via `cargo create-tauri-app -y -t vanilla -m cargo`, with a
`mise.toml` declaring `cargo:tauri-cli = "2"` and a `[tasks.build]` that
runs `cargo tauri build`. Use this for full-pipeline validation:

```bash
cd ~/workspace/go/src/github.com/joeblew999/utm-dev-demo
utm-dev vm build --name linux-build     # produces .deb/.AppImage in .build/linux/
utm-dev vm build --name windows-build   # produces .msi/.exe in .build/windows/
```

When debugging build failures: outputs land in the demo's `.build/<platform>/`
inside the project dir you ran `vm build` from (`current_dir()` at command
start). Open the demo in VS Code and the artifacts appear in the same
workspace.
