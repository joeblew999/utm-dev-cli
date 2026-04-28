# utm-dev

Cross-platform Tauri builds on Apple Silicon. Spins up UTM VMs and ships you `.msi/.exe/.deb/.AppImage` artifacts.

## Install

Via mise (one-liner once a release is tagged):

```sh
mise use --global "ubi:joeblew999/utm-dev-cli@latest"
```

Or from source:

```sh
cargo install --git https://github.com/joeblew999/utm-dev-cli
```

## Quick start (5 min if a pre-baked box exists; ~30 min on cold first run)

```sh
# 1. From your Tauri project root, ensure mise.toml has the toolchain pinned.
#    (Skip if you already have mise.toml; otherwise:)
utm-dev init                # writes a minimal [tools] block

# 2. Sanity-check your host has what you need.
utm-dev doctor

# 3. Cross-build for Windows + Linux from your Mac.
utm-dev windows build       # → .build/windows/x86_64/{msi,nsis}/...msi/.exe
utm-dev linux build         # → .build/linux/arm64/{deb,appimage}/...

# 4. Launch the built app on the VM and watch startup.
utm-dev vm run --name linux-build           # auto-detects the binary
utm-dev vm logs --name linux-build --kind run --follow

# 5. Capture a screenshot of the VM display (Linux GUI only).
utm-dev vm screenshot --name linux-build --out demo.png
```

If something fails, the build CLI prints the error stanza inline and bails fast. For deeper digging:

```sh
utm-dev vm doctor --name <vm>          # canned health checks inside the VM
utm-dev vm logs   --name <vm> --errors # grep error stanzas with context
utm-dev vm logs   --name <vm> --tail 200
```

## Supported targets

| Host VM             | Targets                                  | Notes |
|---------------------|------------------------------------------|---|
| Windows ARM64       | `x86_64-pc-windows-msvc`                 | Native ARM64 blocked on MS toolchain (VS Build Tools doesn't ship `Hostarm64\arm64`). x64 binaries run under Windows ARM64 emulation. |
| Linux ARM64 (Ubuntu)| `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu` | x86_64 via Debian multiarch; installed on demand by `vm build --target x86-64`. |

The same Windows VM produces the x86_64 .msi/.exe — no second VM, no extra disk. Linux x86_64 cross-compile pulls ~500 MB–1 GB of `:amd64` system libs the first time.

## Distribution: pre-baked boxes

Bootstrap is a 25–30 min one-time cost per VM (VS Build Tools, mise, cargo-binstall, multiarch libs, etc.). For team onboarding, package the bootstrapped VM and host the resulting `.box` so new devs download it instead of rebuilding it:

```sh
utm-dev vm package --name windows-build         # → .build/boxes/<box>.box
# upload the .box anywhere with a public URL (Cloudflare R2 recommended)
# set profile.prebaked_url in src/vm/profiles.rs to point at it
```

`utm-dev vm up` will then download the pre-baked box and skip the bootstrap. New-machine onboarding goes from ~30 min to ~5 min. See [docs/box-publishing.md](docs/box-publishing.md).

## Speed-ups baked in

- **mise + cargo-binstall**: prebuilt binaries from GitHub Releases (notably tauri-cli — 30 sec download vs 25 min compile). Pre-installed by bootstrap; mise honours `cargo_binstall = true` from `~/.config/mise/config.toml` (or env `MISE_CARGO_BINSTALL=true`).
- **sccache**: cross-project artifact cache. After first compile, common Tauri lib deps (e.g. `tauri-plugin-fs`) hit cache and don't recompile across projects on the same VM. Wired up in `vm build` via `RUSTC_WRAPPER` — best-effort, no-op if sccache isn't installed.
- **`CARGO_INCREMENTAL=0`**: improves sccache hit rate.
- **VM-wide cargo cache**: `D:\target` (Windows) / project-local on Linux is preserved across `vm build` runs.

## Lower-level commands

```sh
utm-dev vm ls                                       # list profiles + UTM status
utm-dev vm up      --name windows-build             # start + bootstrap
utm-dev vm down    --name windows-build
utm-dev vm restart --name windows-build
utm-dev vm shell   --name windows-build             # interactive ssh
utm-dev vm doctor  --name windows-build             # health checks inside the VM
utm-dev vm exec    --name windows-build  "ver"      # run one command via ssh
utm-dev vm push    --name windows-build  --from ./local --to /vm/path
utm-dev vm pull    --name windows-build  --from /vm/path --to ./local
utm-dev vm package --name windows-build             # export as Vagrant .box
utm-dev vm resize-disk --name windows-build --plus-gb 60
```

## Requirements

- macOS on Apple Silicon
- UTM (auto-installed via Homebrew on first `vm up` if missing)
- An SSH keypair in `~/.ssh/` for passwordless `ssh` and VS Code Remote SSH against the VMs
- Your project must have a `mise.toml` declaring its toolchain — `rust = "stable"` and `"cargo:tauri-cli" = "2"` at minimum. Run `utm-dev init` if you don't have one.

## License

MIT
