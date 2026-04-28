# utm-dev

Cross-platform Tauri builds on Apple Silicon. Spins up UTM VMs and ships you `.msi/.exe/.deb/.AppImage` artifacts.

## Install

Via mise (recommended once a release is tagged — pre-built mac-arm64 binary):

```sh
mise use --global "ubi:joeblew999/utm-dev-cli@latest"
```

Or from source (always works, ~30s compile):

```sh
cargo install --git https://github.com/joeblew999/utm-dev-cli
```

## Use

In your Tauri project:

```sh
utm-dev doctor                # check tools
utm-dev windows build         # x86_64 .msi/.exe → .build/windows/x86_64/
utm-dev linux build           # ARM64 .deb/.AppImage → .build/linux/arm64/
utm-dev all build             # mac + windows + linux + android + ios
```

First run downloads a UTM box and bootstraps the VM (10–20 min). Subsequent builds reuse the VM (~minutes).

## Supported targets

| Host VM             | Targets                                  | Notes |
|---------------------|------------------------------------------|---|
| Windows ARM64       | `x86_64-pc-windows-msvc`                 | Native ARM64 not yet supported (VS Build Tools doesn't ship Hostarm64\arm64 cross-tools); x64 binaries run under Windows ARM64 emulation |
| Linux ARM64 (Ubuntu)| `aarch64-unknown-linux-gnu`              | x86_64 cross not yet supported (multiarch system libs) |

## Lower-level commands

```sh
utm-dev vm ls                                       # list profiles + UTM status
utm-dev vm up      --name windows-build             # start + bootstrap
utm-dev vm down    --name windows-build
utm-dev vm restart --name windows-build
utm-dev vm shell   --name windows-build             # interactive ssh
utm-dev vm doctor  --name windows-build             # health checks inside the VM
utm-dev vm exec    --name windows-build  "ver"      # run one command via ssh

# Logs (build or run; use --tail/--errors when something failed)
utm-dev vm logs  --name windows-build --follow
utm-dev vm logs  --name windows-build --tail 200
utm-dev vm logs  --name windows-build --errors      # grep error stanzas with context
utm-dev vm logs  --name windows-build --kind run --follow

# After build, launch the binary on the VM and capture startup
utm-dev vm run   --name windows-build --bin <vm-side-path-to.exe>
```

## Requirements

- macOS on Apple Silicon
- UTM (auto-installed via Homebrew if missing)
- An SSH keypair in `~/.ssh/` for passwordless `ssh` and VS Code Remote SSH against the VMs

## License

MIT
