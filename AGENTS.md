# Agent Guidelines: utm-dev-cli

Rust CLI rewrite of [utm-dev](https://github.com/joeblew999/utm-dev) TypeScript tasks.
Binary name: `utm-dev`. Published to crates.io so consuming repos install via `cargo:utm-dev`.

## Goal

Once complete, thin bash wrappers in `joeblew999/.github//mise-tasks/vm/` will delegate to
this binary. All repos get cross-platform VM builds via the single `.github` include — no
separate utm-dev TypeScript include needed.

## Architecture

```
src/
  main.rs          entry point — calls cli::run(), prints errors
  cli.rs           clap CLI definition (Commands enum)
  cmd/
    mod.rs
    doctor.rs      ✓ implemented — checks tool availability via which()
    platform.rs    stubs — mac/ios/android/windows/linux/all subcommands
    vm.rs          stubs — vm up/down/build/exec/delete/package
```

## Command surface

```
utm-dev doctor                          # check tools
utm-dev setup                           # install platform deps
utm-dev init                            # scaffold mise.toml
utm-dev mac dev|build
utm-dev ios sim|xcode|build
utm-dev android sim|studio|build
utm-dev windows build [--release]       # delegates to vm build --name windows-11
utm-dev linux dev|build [--release]     # delegates to vm build --name ubuntu-24.04
utm-dev all build
utm-dev vm up|down|build|exec|delete|package --name <profile>
utm-dev clean [--deep]
utm-dev icon
```

## Implementation order

1. `doctor` ✓ — read-only, validates the pattern
2. `vm up` / `vm down` — SSH + utmctl foundation everything else needs
3. `vm exec` — SSH command execution
4. `vm build` — sync code + run cargo tauri build + pull artifacts
5. `windows build` / `linux build` — thin wrappers around vm build
6. `setup` / `init` / `doctor` enhancements

## Key dependencies

- `clap` (derive) — CLI parsing
- `which` — tool detection (doctor)
- `ssh2` — VM SSH (vm exec, vm build) — add when implementing vm commands
- `reqwest` — WinRM bootstrap for Windows VM — add when implementing vm up for windows
- `serde` / `serde_json` — VM state files (~/.cache/utm-dev/vm-<name>.json)
- `anyhow` — error propagation

## VM state model

Port from TypeScript `_lib.ts`. State persisted to `~/.cache/utm-dev/vm-<name>.json`:
- `uuid` — UTM VM UUID (set after import)
- `display_name` — UTM display name
- `ssh_host` / `ssh_port` / `ssh_user` / `ssh_password`
- `status` — running | stopped | not-imported

## Conventions

- All commands return `anyhow::Result<()>`
- Print progress with `println!("→ ...")`, success with `"✓ ..."`, errors with `"✗ ..."`
- Unimplemented commands use `todo!("...")` — they panic with a clear message
- Never `unwrap()` in user-facing paths — propagate with `?` or `anyhow::bail!`
