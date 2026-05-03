# utm-dev

Cross-platform Rust + Tauri builds **on Apple Silicon**. Runs `cargo build`
or `cargo tauri build` for Windows (x86_64) and Linux (arm64/x86_64) inside
managed UTM VMs and pulls the artifacts back to your Mac.

```sh
utm-dev windows build      # → .build/windows/x86_64/<bin>.exe (or .msi for Tauri)
utm-dev linux   build      # → .build/linux/arm64/<bin>       (or .deb / .AppImage)
```

Detects whether your project is Tauri (has `src-tauri/`) or plain cargo and
dispatches accordingly. No setup beyond `mise.toml` with `[tools] rust = "..."`.

## Requirements

- macOS on Apple Silicon (M1+)
- [UTM](https://mac.getutm.app/) — auto-installed via Homebrew on first `vm up`
- ED25519 SSH key at `~/.ssh/id_ed25519` (or `~/.ssh/utm_id_ed25519`)
- [mise](https://mise.jdx.dev/) on the host AND a `mise.toml` in your project

## Install

```sh
mise use --global "ubi:joeblew999/utm-dev-cli@latest"   # released versions
cargo install --git https://github.com/joeblew999/utm-dev-cli   # from source
```

## Quick start

```sh
utm-dev init                # scaffolds mise.toml if missing
utm-dev doctor              # sanity-check the host
utm-dev windows build       # cross-build for Windows
utm-dev linux   build       # cross-build for Linux
```

First run on a fresh VM is ~25 min (one-time bootstrap of Build Tools, mise,
sccache, etc.). Subsequent builds are 3–5 min cold, seconds warm. Tail the
in-VM build log from another terminal:

```sh
utm-dev vm logs --name windows-build --kind run --follow
utm-dev vm logs --name windows-build --errors
```

## Commands

```sh
utm-dev vm ls                                       # list profiles + UTM status
utm-dev vm up      --name windows-build             # start + bootstrap
utm-dev vm down    --name windows-build
utm-dev vm doctor  --name windows-build             # in-VM health checks
utm-dev vm shell   --name windows-build             # interactive ssh
utm-dev vm exec    --name windows-build -- "ver"    # run one command
utm-dev vm run     --name X --bin foo.exe -- --version    # launch + capture stdout (CLI)
utm-dev vm run     --name X --bin app.exe --interactive   # launch GUI app in logged-on desktop
utm-dev vm screenshot --name X --out app.png              # PNG capture (Linux: scrot/Xvfb; Windows: in-VM)
utm-dev vm push    --name X --from ./local --to /vm/path
utm-dev vm pull    --name X --from /vm/path --to ./local
utm-dev vm logs    --name X --kind run --tail 50    # tail captured stdout/stderr
utm-dev vm clean   --name X                         # reclaim disk
utm-dev vm package --name X                         # export as Vagrant .box
utm-dev vm resize-disk    --name X --plus-gb 30
utm-dev vm refresh-network --name X                 # recover stale port forwards
```

UI regression on the host:

```sh
utm-dev screenshot --out app.png                          # capture Tauri WebView
utm-dev validate --actual app.png --golden golden.png    # pixel-diff vs golden
```

## Pre-baked boxes

The 25-min bootstrap is a one-time cost per VM. Package the result as a
`.box`, host it somewhere with a public URL, and `vm up` will skip the
bootstrap. New-machine onboarding goes from ~30 min to ~5 min. See
[docs/box-publishing.md](docs/box-publishing.md).

## Caveats

- **Windows VM C: fills up.** Vagrant's `utm/windows-11` ships a 26 GB C:; VS
  Build Tools eat ~25 GB. Use `vm clean` or `vm resize-disk --plus-gb 30`.
- **Tauri Windows release builds exit immediately under `vm run`** — Win32
  GUI subsystem has no stdout. Use RDP at `localhost:3389` (vagrant/vagrant)
  or have the app POST startup events to your own logger.
- **Linux x86_64 cross-build pulls ~1 GB of `:amd64` libs on first run.**
  Cached after that.
- **No native Windows ARM64 link target.** VS Build Tools doesn't ship
  `Hostarm64\arm64\link.exe`, so we cross to x86_64 and let Windows ARM64
  emulate.

Full punch list: [GAPS.md](GAPS.md). Smoke tests: `mise run e2e:smoke`
(VM-touching) or `mise run e2e:fast` (host-only, ~25s).

## License

MIT
