# utm-dev

Cross-platform builds **on Apple Silicon** for Rust projects — both Tauri apps
(produces `.msi`/`.exe`/`.deb`/`.AppImage` bundles) and plain Rust binaries
(produces a single executable per target). Manages UTM VMs for you.

## What it actually does (under the hood)

When you run `utm-dev windows build` from your Mac:

1. **Detects project kind** — looks for `src-tauri/` in the project. If
   present → Tauri build. Otherwise → plain `cargo build --release`.
2. **Pre-flights** your `mise.toml` — fails fast if `rust` isn't pinned (or
   `cargo:tauri-cli` for Tauri).
3. **Boots the VM** (UTM `windows-build` or `linux-build`) — auto-imports
   from a Vagrant `.box` and bootstraps on first run (~25 min one-time;
   ~10 sec on subsequent runs once warm).
4. **Tars your project** (excluding `target/`, `node_modules/`, `.git/`, etc),
   uploads via SCP, untars on the VM.
5. **Runs `mise install` inside the VM** — provisions Rust, tauri-cli (if
   needed), sccache, cargo-binstall to the versions your `mise.toml` pins.
6. **Builds**: `cargo tauri build --target <triple>` (Tauri) or
   `cargo build --release --target <triple>` (plain). Uses sccache for
   cross-project artifact caching, sccache-friendly env (`CARGO_INCREMENTAL=0`,
   `RUSTC_WRAPPER=sccache`), and on Windows wraps with
   `VsDevCmd -arch=amd64 -host_arch=arm64` because there's no native
   ARM64-host ARM64-targeting MSVC linker.
7. **Pulls artifacts back** — for Tauri, tars the entire `bundle/` directory
   (multiple files: .msi + .exe + .nsis on Windows, .deb + .AppImage on
   Linux). For plain cargo, scp's the single binary at
   `target/<triple>/release/<bin-name>[.exe]`.

Output lands in `.build/<platform>/<arch>/` in your project. **No artifacts
on the host's macOS toolchain are touched** — your local `target/` is
unaffected because the VM has its own.

## What it assumes

| | |
|---|---|
| **Host OS** | macOS on Apple Silicon (M1+). Won't run on Intel Mac (UTM works there but the VM profiles are ARM64-only) or any non-Mac. |
| **UTM** | Installed at `/Applications/UTM.app` and `utmctl` on PATH. Auto-installed via Homebrew on first `vm up` if missing. |
| **SSH** | An ED25519 keypair at `~/.ssh/id_ed25519` (or `~/.ssh/utm_id_ed25519`). VMs are bootstrapped with the public key in `authorized_keys`. |
| **mise** | On the host (for `vm:up` to read profile config) AND in every VM (provisioned by bootstrap, drives Rust toolchain pins). |
| **Project layout** | A `mise.toml` at the project root with at least `[tools] rust = "stable"`. Tauri projects also need `"cargo:tauri-cli" = "2"`. Run `utm-dev init` to scaffold one. |
| **Cargo.toml** | Required for plain Rust builds — `[package].name` is read to know which binary file to pull back from the VM. |
| **VMs are ARM64** | The Windows VM is Win11 ARM64 (cross-compiles to x86_64 — Microsoft's toolchain doesn't ship native ARM64-on-ARM64 host MSVC linker). The Linux VM is Ubuntu ARM64 (with optional `:amd64` multiarch for x86_64 cross-builds). |

## Install

Via mise (when a release is tagged):

```sh
mise use --global "ubi:joeblew999/utm-dev-cli@latest"
```

From source:

```sh
cargo install --git https://github.com/joeblew999/utm-dev-cli
```

## Quick start

### Tauri app

```sh
# 1. Make sure mise.toml has rust + cargo:tauri-cli
utm-dev init                # scaffolds a starter [tools] block

# 2. Sanity check the host
utm-dev doctor

# 3. Cross-build for Windows + Linux
utm-dev windows build       # → .build/windows/x86_64/{msi,nsis}/...msi/.exe
utm-dev linux build         # → .build/linux/arm64/{deb,appimage}/...
```

### Plain Rust binary (any cargo project)

Same commands, no Tauri boilerplate needed:

```sh
# Project just needs Cargo.toml + mise.toml with [tools] rust = "stable"
utm-dev windows build       # → .build/windows/x86_64/<your-binary>.exe
utm-dev linux build         # → .build/linux/arm64/<your-binary>
```

`utm-dev` detects `src-tauri/` and dispatches accordingly. **The same `vm build`
flow handles both** — only the `cargo` subcommand and the artifact path
differ. (Implemented as a `ProjectKind` branch in [`src/vm/build.rs`](src/vm/build.rs).)

### Watching the build

First builds are slow (10–30 min — cargo-binstall pulls tauri-cli in <1 min,
but the project's Rust deps still compile from source). Tail the persistent
build log from another terminal:

```sh
utm-dev vm logs --name windows-build --kind run --follow
utm-dev vm logs --name windows-build --errors    # grep error stanzas with context
```

If the build fails, `utm-dev` prints the last error stanza inline before
bailing. For deeper digging:

```sh
utm-dev vm doctor --name windows-build         # canned in-VM health checks
utm-dev vm shell  --name windows-build         # drop into the VM via ssh
```

## Supported targets

| Host VM             | Targets                                               | Notes |
|---------------------|-------------------------------------------------------|-------|
| Windows ARM64       | `x86_64-pc-windows-msvc`                              | Native ARM64 blocked: VS Build Tools doesn't ship `Hostarm64\arm64\link.exe`. x64 binaries run under Windows ARM64 emulation. |
| Linux ARM64 (Ubuntu)| `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu` | x86_64 cross-build pulls ~1 GB of `:amd64` multiarch system libs the first time. |

The same Windows VM produces the x86_64 `.msi`/`.exe` artifact — no second
VM, no extra disk.

## Distribution: pre-baked boxes

Bootstrap is a 25–30 min one-time cost per VM (VS Build Tools, mise, cargo-binstall,
multiarch libs, etc.). For team onboarding, package the bootstrapped VM and host
the resulting `.box`:

```sh
utm-dev vm package --name windows-build         # → .build/boxes/<box>.box
# upload the .box anywhere with a public URL (Cloudflare R2 recommended)
# set profile.prebaked_url in src/vm/profiles.rs to point at it
```

`utm-dev vm up` will download the pre-baked box and skip the bootstrap. New-machine
onboarding goes from ~30 min to ~5 min. See [docs/box-publishing.md](docs/box-publishing.md).

## Speed-ups baked in

| | |
|---|---|
| **`mise + cargo-binstall`** | Prebuilt binaries from GitHub Releases — notably tauri-cli (~30 sec download vs ~25 min compile). Bootstrap pre-installs cargo-binstall and `~/.config/mise/config.toml` sets `cargo_binstall = true`. |
| **`sccache`** | Cross-project Rust artifact cache. After the first compile, common deps (`serde`, `tauri-plugin-fs`, etc) hit cache across projects on the same VM. Wired up via `RUSTC_WRAPPER` — best-effort, no-op if sccache isn't installed. |
| **`CARGO_INCREMENTAL=0`** | Disables incremental compilation; sccache works much better in non-incremental mode (incremental hashes change across projects, breaking cache). |
| **VM-wide target cache** | `D:\target` on Windows (extra qcow2 disk) / project-local on Linux. Preserved across `vm build` runs — only changed crates recompile. |

## Lower-level commands

```sh
utm-dev vm ls                                       # list profiles + UTM status
utm-dev vm up      --name windows-build             # start + bootstrap
utm-dev vm down    --name windows-build
utm-dev vm restart --name windows-build
utm-dev vm shell   --name windows-build             # interactive ssh
utm-dev vm doctor  --name windows-build             # in-VM health checks
utm-dev vm clean   --name windows-build             # reclaim disk (logs, installers)
utm-dev vm exec    --name windows-build  -- "ver"   # run one command via ssh
utm-dev vm push    --name windows-build  --from ./local --to /vm/path
utm-dev vm pull    --name windows-build  --from /vm/path --to ./local
utm-dev vm package --name windows-build             # export as Vagrant .box
utm-dev vm resize-disk --name windows-build --plus-gb 30  # grow C: when full
```

## How `utm-dev windows build` differs from doing it manually

You could replicate `utm-dev windows build` with the lower-level primitives:

```sh
utm-dev vm up      --name windows-build
utm-dev vm push    --name windows-build --from . --to /Users/vagrant/myproject
utm-dev vm exec    --name windows-build -- "cd myproject && mise install && \
    mise exec -- cargo build --release --target x86_64-pc-windows-msvc"
utm-dev vm pull    --name windows-build \
    --from "C:\\Users\\vagrant\\myproject\\target\\x86_64-pc-windows-msvc\\release\\myapp.exe" \
    --to ./dist/
```

The high-level `windows build` wraps that — adds: VS Build Tools env activation,
sccache wiring, mise-version pinning, multiarch setup (Linux), persistent in-VM
build log with error grep, and tar-based directory pull (Tauri bundles).
**Use the low-level primitives only if you need behavior the high-level
command doesn't support yet** (file an issue).

## Known caveats

- **Windows VM C: fills up.** Vagrant's `utm/windows-11` ships a 26 GB C:; VS
  Build Tools + Windows leave ~0.5 GB free. Run
  `utm-dev vm clean --name windows-build` to reclaim some space, or
  `vm resize-disk --plus-gb 30` for headroom.
- **Tauri Windows release builds exit in headless `vm run`** — Win32 GUI
  subsystem has no stdout and exits without an interactive desktop. Use RDP
  at `localhost:3389` (user/pass `vagrant`/`vagrant`) for visual access, or
  embed an out-of-band logger per
  [docs/adr-001](docs/adr-001-vm-run-observability.md).
- **`vm screenshot` of Tauri Linux is a black PNG** — WebKit-GTK needs GL/EGL;
  bare Xvfb has none. Process-level verification works; for visual capture
  use the logger pattern.
- **Linux x86_64 cross-build pulls ~1 GB of `:amd64` libs on first run.**
  Cached after that. `--target x86_64` on Linux can take 5–10 min just for
  the multiarch apt-get the first time.

See [GAPS.md](GAPS.md) for the full punch list.

## What's been verified (and what hasn't)

Honest status as of the plain-cargo extension landing:

| Layer | Status | Notes |
|---|---|---|
| Code compiles | ✓ | `cargo build --release` clean |
| Existing Tauri code path | ✓ — untouched | All original code retained verbatim; `ProjectKind::Tauri` branch identical to pre-change behavior. `joeblew999/ifc-lite` continues to build as before. |
| `ProjectKind` detection | ✓ — confirmed live | Ran `utm-dev windows build --release` against this very repo; first line of output is `→ Project kind: cargo`. The dispatch correctly classifies utm-dev-cli (no `src-tauri/`) as plain cargo. |
| Pre-flight (`mise.toml` requirements per kind) | ✓ — code path proven by dispatch | Plain cargo only requires `rust`; Tauri requires `rust + cargo:tauri-cli`. |
| Plain-cargo `cargo build` step end-to-end | ✗ — **not yet reached** in real run | The dogfood test against utm-dev-cli itself failed at the **`mise install` step inside the VM** with a pre-existing PowerShell quoting issue (`MissingEndCurlyBrace`). This is unrelated to the plain-cargo extension — same bug exists for Tauri builds on a fresh windows-build VM. The cargo build branch wasn't reached. See [GAPS.md](GAPS.md). |
| Linux end-to-end (Tauri or plain cargo) | known-good for Tauri; plain-cargo untested |

**Net**: the new dispatch + helper code is correct and runs in production
context. The cross-compile artifact pull path for plain cargo is **untested
end-to-end on a real VM yet** — pending fix of the PowerShell install-step
bug, then a re-run.

## Dogfood test (run this to verify)

utm-dev itself is a plain Rust project — perfect for verifying the
plain-cargo path end-to-end. From this repo's root:

```sh
cargo build --release                       # build utm-dev locally
./target/release/utm-dev windows build --release
                                            # uses the new utm-dev to cross-build
                                            # itself on the windows-build VM.
                                            # ~25 min first time (mise installs
                                            # rust + sccache + cargo-binstall on
                                            # the VM); ~3-5 min on subsequent runs.

ls .build/windows/x86_64/utm-dev.exe        # the cross-built ARM64-host →
                                            # x86_64-Windows binary
```

If that produces a runnable `utm-dev.exe` (you can verify by `vm push`-ing
it to a separate Windows-test VM and running it), the plain-cargo path is
verified end-to-end on your machine.

For Tauri-path verification, use any Tauri project — `joeblew999/ifc-lite`
is the reference.

## License

MIT
