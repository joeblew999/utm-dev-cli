/// Sync project to VM, run `cargo tauri build --target <triple>` for each
/// requested architecture, pull artifacts back.
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::cli::BuildTarget;
use super::profiles::{GuestOs, VmProfile};
use super::ssh;

/// Format an elapsed duration as `Hh Mm Ss` or `Mm Ss` or `Ss`.
fn fmt_elapsed(d: std::time::Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}h {}m {}s", s / 3600, (s % 3600) / 60, s % 60) }
    else if s >= 60 { format!("{}m {}s", s / 60, s % 60) }
    else { format!("{}s", s) }
}

/// Rust target triples for each (BuildTarget × GuestOs) combination.
///
/// Windows arm64 / both are intentionally absent — the caller (`vm_build`)
/// rejects them up front because Microsoft's VS Build Tools doesn't ship a
/// native ARM64-host-targeting-ARM64 toolchain (Hostarm64\arm64\link.exe).
/// Only ARM64-host cross-compile to x64/x86 is available, so we ship x64
/// binaries from the ARM64 VM via x64 emulation.
///
/// Linux x86_64 is also rejected — needs Debian multiarch (libwebkit2gtk:amd64
/// + gcc-x86-64-linux-gnu) which we don't yet provision.
fn triples(target: BuildTarget, os: GuestOs) -> Vec<&'static str> {
    match (target, os) {
        (BuildTarget::X8664,  GuestOs::Windows) => vec!["x86_64-pc-windows-msvc"],
        // Should never reach: vm_build bails for Windows arm64/both, Linux x86_64/both
        (_,                   GuestOs::Windows) => vec!["x86_64-pc-windows-msvc"],
        (BuildTarget::Arm64,  GuestOs::Linux)   => vec!["aarch64-unknown-linux-gnu"],
        (_,                   GuestOs::Linux)   => vec!["aarch64-unknown-linux-gnu"],
    }
}

pub fn run(profile: &VmProfile, project_dir: &Path, target: BuildTarget) -> Result<()> {
    let total_start = Instant::now();

    // Pre-flight: verify the project's mise.toml declares the toolchain we need.
    // Without rust + tauri-cli pinned, mise install inside the VM will either
    // skip what we need or compile against the wrong default. Bail in 50 ms
    // here instead of after 25 min in the VM.
    preflight_mise_toml(project_dir)?;
    let project_name = project_dir
        .file_name()
        .context("project dir has no name")?
        .to_string_lossy()
        .to_string();

    let sep = match profile.os {
        GuestOs::Windows => '\\',
        GuestOs::Linux   => '/',
    };
    let vm_home = match profile.os {
        GuestOs::Windows => format!("C:\\Users\\{}", profile.user),
        GuestOs::Linux   => format!("/home/{}", profile.user),
    };
    let vm_project_dir = format!("{vm_home}{sep}{project_name}");

    let platform      = match profile.os { GuestOs::Windows => "windows", GuestOs::Linux => "linux" };
    let artifacts_dir = project_dir.join(".build").join(platform);

    // ── Connect ───────────────────────────────────────────────────────────────

    println!("→ Connecting to {} VM...", profile.name);
    let t = Instant::now();
    let session = ssh::connect(profile)?;
    println!("  ⌚ connect: {}", fmt_elapsed(t.elapsed()));

    // ── Sync code ─────────────────────────────────────────────────────────────

    let t_sync = Instant::now();
    println!("→ Syncing {} to VM...", project_name);
    let tmp_tar = std::env::temp_dir().join(format!("utm-dev-sync-{}.tar.gz", std::process::id()));

    // Exclude macOS AppleDouble files (._*) — they're HFS metadata bytes
    // that aren't valid UTF-8, and Tauri's build script reads everything
    // in src-tauri/capabilities/ which would crash on them. Also tell tar
    // not to create them via COPYFILE_DISABLE.
    let status = Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .args([
            "-czf",
            tmp_tar.to_str().unwrap(),
            "--exclude=target",
            "--exclude=node_modules",
            "--exclude=.git",
            "--exclude=.mise/logs",
            "--exclude=.mise/state",
            "--exclude=.build",
            "--exclude=.gradle",
            "--exclude=._*",
            "--exclude=.DS_Store",
            "-C",
            project_dir.to_str().unwrap(),
            ".",
        ])
        .status()
        .context("running tar to create sync archive")?;
    if !status.success() {
        bail!("tar failed to create sync archive");
    }

    let tar_bytes = std::fs::metadata(&tmp_tar)?.len();
    println!("  archive: {:.1} MB", tar_bytes as f64 / 1_048_576.0);

    // SCP destination: absolute Unix-style path on Linux; relative path on
    // Windows (libssh2 doesn't translate `C:/...` to a Windows path that
    // OpenSSH-SCP accepts — relative lands in the user's home directory).
    let remote_tar = match profile.os {
        GuestOs::Linux   => format!("/home/{}/sync.tar.gz", profile.user),
        GuestOs::Windows => "sync.tar.gz".to_string(),
    };
    ssh::upload(profile, &tmp_tar, &remote_tar)?;
    let _ = std::fs::remove_file(&tmp_tar);
    println!("  ✓ uploaded");

    // Untar on VM
    if profile.os == GuestOs::Linux {
        let (out, code) = ssh::exec_with_exit(
            &session,
            &format!(
                r#"mkdir -p "{vm_project_dir}" && cd "{vm_project_dir}" && tar -xzf ~/sync.tar.gz && rm ~/sync.tar.gz"#
            ),
        )?;
        if code != 0 { bail!("untar failed on VM:\n{out}"); }
    } else {
        // cmd.exe gotcha: `if not exist X CMD1 && CMD2` parses as
        // `if not exist X (CMD1 && CMD2)` — so on a re-run where X already
        // exists, NONE of the chain runs. Use unconditional `&` plus
        // `mkdir 2>nul` to swallow the "exists" error.
        let (out, code) = ssh::exec_with_exit(
            &session,
            &format!(
                r#"mkdir "{vm_project_dir}" 2>nul & cd /d "{vm_project_dir}" && tar -xzf "%USERPROFILE%\sync.tar.gz" && del "%USERPROFILE%\sync.tar.gz""#
            ),
        )?;
        if code != 0 { bail!("untar failed on VM:\n{out}"); }
    }
    println!("✓ Code synced  ⌚ sync: {}", fmt_elapsed(t_sync.elapsed()));

    // ── Install tools ─────────────────────────────────────────────────────────

    let mise  = if profile.os == GuestOs::Linux { "~/.local/bin/mise" } else { "mise" };

    // Persistent log on the VM. `vm logs --name X` tails this.
    // - Linux: bash `exec > >(tee -a)` redirects stdout/stderr while the
    //   command's exit code is preserved.
    // - Windows: cmd.exe has no native tee, and piping through PowerShell
    //   Tee-Object swallows the inner command's exit code (the pipe
    //   returns powershell's exit, not cmd's). So on Windows we redirect
    //   to the log file with `> log 2>&1` — loses live host-side streaming
    //   for that exact command, but exit codes propagate correctly. Use
    //   `vm logs --name X --follow` from a second terminal for live tail.
    let log_path_h = match profile.os {
        GuestOs::Linux   => "~/.utm-dev-build/build.log",
        GuestOs::Windows => r"%USERPROFILE%\.utm-dev-build\build.log",
    };
    let mkdir_log = match profile.os {
        GuestOs::Linux   => "mkdir -p ~/.utm-dev-build".to_string(),
        GuestOs::Windows => r#"(if not exist "%USERPROFILE%\.utm-dev-build" mkdir "%USERPROFILE%\.utm-dev-build")"#.to_string(),
    };
    let linux_tee = "exec > >(tee -a ~/.utm-dev-build/build.log) 2>&1; ";

    // VsDevCmd path on Windows. We invoke it with -arch=amd64 -host_arch=arm64
    // because the VM is ARM64 but ships only the x64-target cross-tools
    // (Hostarm64\x64\link.exe) — there's no Hostarm64\arm64 toolchain in
    // current VS Build Tools releases for ARM64 hosts. Result: every Rust
    // build on this VM produces x86_64 binaries that run under Windows ARM64
    // x64 emulation, including the tauri-cli compiled by mise install.
    let vsdevcmd = r#"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat"#;
    let win_msvc_x64 = format!(r#"call "{vsdevcmd}" -arch=amd64 -host_arch=arm64 -no_logo"#);

    let t_install = Instant::now();
    println!("→ Running mise install in VM (persistent log at {log_path_h})...");
    let install_cmd = if profile.os == GuestOs::Windows {
        // Two-phase mise install on ARM64 Windows:
        //   1. `mise install rust` — installs rustup + the project's pinned
        //      rust version. mise's rust plugin defers to rustup, which
        //      defaults to the host arch (aarch64-pc-windows-msvc).
        //   2. Switch rustup's default-host + active toolchain to x86_64.
        //      The aarch64 toolchain stays installed but inactive.
        //   3. `mise install` (rest) — anything that compiles via cargo
        //      (notably cargo:tauri-cli) now uses the x86_64 toolchain and
        //      links cleanly with Hostarm64\x64\link.exe.
        //
        // Why this is needed: Microsoft's VS Build Tools doesn't ship a
        // native ARM64-host-targeting-ARM64 MSVC toolchain
        // (Hostarm64\arm64\link.exe). Without that, an aarch64 Rust
        // toolchain has no linker. We pivot the entire build to x64 and
        // run under Windows ARM64's native x64 emulation. The .msi/.exe
        // produced are x86_64 — what most Windows users actually ship.
        let switch_rustup = r#"powershell -NoProfile -Command "$rustup = (& mise where rust 2>$null) + '\\rustup.exe'; if (Test-Path $rustup) { & $rustup set default-host x86_64-pc-windows-msvc; & $rustup default --force-non-host stable-x86_64-pc-windows-msvc }""#;
        format!(
            r#"{mkdir_log} & cd /d "{vm_project_dir}" && ({win_msvc_x64} && {mise} trust --yes && {mise} install rust && {switch_rustup} && {mise} install) >> "{log_path_h}" 2>&1"#
        )
    } else {
        format!(r#"{mkdir_log} && {linux_tee}cd "{vm_project_dir}" && {mise} trust --yes && {mise} install"#)
    };
    let code = ssh::exec_streaming(profile, &install_cmd)?;
    if code != 0 {
        dump_build_log_errors(profile);
        bail!(
            "mise install failed inside VM (exit {code}). Full log: utm-dev vm logs --name {}",
            profile.name
        );
    }
    println!("✓ Tools installed  ⌚ mise install: {}", fmt_elapsed(t_install.elapsed()));

    // Linux x86_64 cross-compile prep: ensure multiarch + amd64 system libs.
    // No-op on Windows or when only building native ARM64 on Linux.
    if profile.os == GuestOs::Linux
        && (target == BuildTarget::X8664 || target == BuildTarget::Both)
    {
        ensure_linux_multiarch(profile, &session)?;
    }

    // Resolve CARGO_TARGET_DIR once. cargo per-target output goes to
    // {target_dir}/{triple}/release/bundle.
    //
    // Use fenced markers (BEGIN_/END_) so we don't accidentally pick up
    // shell prompts, login banners, or other stray output.
    let probe = if profile.os == GuestOs::Windows {
        r#"echo BEGIN_CTD & if defined CARGO_TARGET_DIR (echo %CARGO_TARGET_DIR%) else (echo DEFAULT) & echo END_CTD"#.to_string()
    } else {
        r#"echo BEGIN_CTD; echo "${CARGO_TARGET_DIR:-DEFAULT}"; echo END_CTD"#.to_string()
    };
    let (probe_out, _) = ssh::exec_with_exit(&session, &probe)?;
    let target_dir_raw = probe_out
        .lines()
        .skip_while(|l| l.trim() != "BEGIN_CTD")
        .nth(1)
        .map(|l| l.trim())
        .unwrap_or("DEFAULT");
    let target_root = if target_dir_raw == "DEFAULT" || target_dir_raw.is_empty() {
        format!("{vm_project_dir}{sep}src-tauri{sep}target")
    } else {
        target_dir_raw.to_string()
    };

    // ── Build per target ──────────────────────────────────────────────────────

    let triples = triples(target, profile.os);
    let platform_label = match profile.os { GuestOs::Windows => "Windows", GuestOs::Linux => "Linux" };
    println!(
        "→ Building Tauri {platform_label} app for: {} (first build per arch may take 10–30 min — tail with: vm logs --name {} --follow)",
        triples.join(", "),
        profile.name,
    );

    for triple in &triples {
        let t_target = Instant::now();
        println!("\n── Target: {triple} ──");

        // 1. rustup target add (idempotent — no-op if already installed).
        //    `mise exec --` runs with the project's mise-managed PATH so the
        //    right rustup/cargo are picked up.
        let target_add_cmd = if profile.os == GuestOs::Windows {
            format!(
                r#"cd /d "{vm_project_dir}" && ({mise} exec -- rustup target add {triple}) >> "{log_path_h}" 2>&1"#
            )
        } else {
            format!(
                r#"{linux_tee}cd "{vm_project_dir}" && {mise} exec -- rustup target add {triple}"#
            )
        };
        let code = ssh::exec_streaming(profile, &target_add_cmd)?;
        if code != 0 {
            bail!("rustup target add {triple} failed (exit {code})");
        }

        // 2. cargo tauri build --target <triple>
        //    On Windows we always use the x64 toolchain (Hostarm64\x64) since
        //    the only supported triple is x86_64-pc-windows-msvc.
        //    For Linux x86_64 cross-compile, point cargo at the multiarch
        //    cross-linker (gcc-x86-64-linux-gnu) and pass PKG_CONFIG_PATH so
        //    pkg-config picks up :amd64 system libs.
        let build_cmd = if profile.os == GuestOs::Windows {
            format!(
                r#"cd /d "{vm_project_dir}" && ({win_msvc_x64} && {mise} exec -- cargo tauri build --target {triple}) >> "{log_path_h}" 2>&1"#
            )
        } else {
            let linux_cross_env = linux_cross_env_for(triple);
            format!(
                r#"{linux_tee}cd "{vm_project_dir}" && {linux_cross_env}{mise} exec -- cargo tauri build --target {triple}"#
            )
        };
        let t_build = Instant::now();
        let code = ssh::exec_streaming(profile, &build_cmd)?;
        if code != 0 {
            dump_build_log_errors(profile);
            bail!(
                "Build failed for {triple} (exit {code}). Full log: utm-dev vm logs --name {}",
                profile.name
            );
        }
        println!("✓ Build complete: {triple}  ⌚ cargo tauri build: {}", fmt_elapsed(t_build.elapsed()));

        // 3. Pull this triple's bundle into .build/{platform}/{arch}/
        let t_pull = Instant::now();
        let arch_label = arch_label_for(triple);
        let arch_dir = artifacts_dir.join(arch_label);
        std::fs::create_dir_all(&arch_dir)?;

        let bundle_path = format!("{target_root}{sep}{triple}{sep}release{sep}bundle");
        println!("  bundle path: {bundle_path}");

        let (out, code) = if profile.os == GuestOs::Linux {
            ssh::exec_with_exit(
                &session,
                &format!(r#"cd "{bundle_path}" && tar -czf ~/artifacts.tar.gz ."#),
            )?
        } else {
            ssh::exec_with_exit(
                &session,
                &format!(r#"cd "{bundle_path}" && tar -czf "%USERPROFILE%\artifacts.tar.gz" ."#),
            )?
        };
        if code != 0 { bail!("Failed to archive artifacts on VM ({triple}):\n{out}"); }

        let remote_artifacts = match profile.os {
            GuestOs::Linux   => format!("/home/{}/artifacts.tar.gz", profile.user),
            GuestOs::Windows => "artifacts.tar.gz".to_string(),
        };
        let local_tar = arch_dir.join("artifacts.tar.gz");
        ssh::download(profile, &remote_artifacts, &local_tar)?;

        let _ = if profile.os == GuestOs::Linux {
            ssh::exec(&session, "rm ~/artifacts.tar.gz")
        } else {
            ssh::exec(&session, r#"del "%USERPROFILE%\artifacts.tar.gz""#)
        };

        let status = Command::new("tar")
            .args([
                "-xzf",
                local_tar.to_str().unwrap(),
                "-C",
                arch_dir.to_str().unwrap(),
            ])
            .status()
            .context("extracting artifacts")?;
        if !status.success() { bail!("Failed to extract artifacts locally for {triple}"); }
        let _ = std::fs::remove_file(&local_tar);

        println!("✓ Artifacts in {}:", arch_dir.display());
        let exts: &[&str] = match profile.os {
            GuestOs::Linux   => &["deb", "AppImage", "rpm"],
            GuestOs::Windows => &["msi", "exe"],
        };
        for path in walk_files(&arch_dir)? {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if exts.contains(&ext) {
                let size = std::fs::metadata(&path)?.len();
                println!("  {} ({:.1} MB)", path.display(), size as f64 / 1_048_576.0);
            }
        }
        println!("  ⌚ pull+extract: {} | target total: {}",
            fmt_elapsed(t_pull.elapsed()),
            fmt_elapsed(t_target.elapsed()),
        );
    }

    println!("\n══ Done. Total: {} ══", fmt_elapsed(total_start.elapsed()));
    Ok(())
}

fn arch_label_for(triple: &str) -> &'static str {
    match triple {
        "aarch64-pc-windows-msvc"     => "arm64",
        "x86_64-pc-windows-msvc"      => "x86_64",
        "aarch64-unknown-linux-gnu"   => "arm64",
        "x86_64-unknown-linux-gnu"    => "x86_64",
        _                              => "unknown",
    }
}

/// Pre-flight validation: ensure the project's mise.toml declares the
/// toolchain we need. We only check for the markers; full TOML parsing
/// adds a dependency that's not worth the cost for a substring match.
fn preflight_mise_toml(project_dir: &Path) -> Result<()> {
    let path = project_dir.join("mise.toml");
    if !path.exists() {
        bail!(
            "no mise.toml in {}. Run `utm-dev init` to scaffold one.",
            project_dir.display()
        );
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;

    let has_rust = content.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("rust ") || t.starts_with("rust=") || t.starts_with("rust\t")
    });
    let has_tauri_cli = content.contains("tauri-cli");

    let mut missing: Vec<&str> = Vec::new();
    if !has_rust { missing.push("rust"); }
    if !has_tauri_cli { missing.push("\"cargo:tauri-cli\""); }
    if !missing.is_empty() {
        bail!(
            "mise.toml is missing required pins: {}.\n  \
             Add to [tools]:\n    \
             rust              = \"stable\"\n    \
             \"cargo:tauri-cli\" = \"2\"\n  \
             Or run: utm-dev init",
            missing.join(", ")
        );
    }
    Ok(())
}

/// On build failure, fetch the tail of the build log from inside the VM
/// and print just the error stanzas. Best-effort — if the VM can't be
/// reached we just skip and let the bail message do the work.
fn dump_build_log_errors(profile: &VmProfile) {
    let log_path = match profile.os {
        GuestOs::Linux   => "~/.utm-dev-build/build.log",
        GuestOs::Windows => r"%USERPROFILE%\.utm-dev-build\build.log",
    };
    let cmd = match profile.os {
        GuestOs::Linux => format!(
            "grep -niE -A 5 -B 1 \
             '(^error[:[ ]|^error\\[E[0-9]+\\]|^FAILED|^Failed |panic|fatal error|mise ERROR|unresolved external symbol|LNK[0-9]+|cannot find -l|linker .* not found)' \
             {log_path} 2>/dev/null | tail -n 80"
        ),
        GuestOs::Windows => format!(
            r#"powershell -NoProfile -Command "if (Test-Path '{log_path}') {{ \
                Get-Content '{log_path}' | Select-String -Pattern '^error[:[ ]|^error\[E[0-9]+\]|^FAILED|panic|fatal error|mise ERROR|unresolved external symbol|LNK[0-9]+|cannot find -l|linker .* not found' -Context 1,5 -CaseSensitive:$false | \
                Select-Object -Last 12 | ForEach-Object {{ $_.Context.PreContext + $_.Line + $_.Context.PostContext + '---' }} \
              }} else {{ '(no build log)' }}""#
        ),
    };
    eprintln!("\n── Last error stanzas (from {} build log) ──", profile.name);
    let _ = ssh::exec_streaming(profile, &cmd);
    eprintln!("─────────────────────────────────────────────────");
}

/// Per-triple env prefix for cargo tauri build on Linux. Currently only
/// the x86_64 cross needs anything — point cargo at the cross-linker and
/// pkg-config at the multiarch :amd64 library paths so libwebkit2gtk and
/// friends resolve correctly.
fn linux_cross_env_for(triple: &str) -> String {
    match triple {
        "x86_64-unknown-linux-gnu" => {
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
             PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig \
             PKG_CONFIG_ALLOW_CROSS=1 \
             PKG_CONFIG_SYSROOT_DIR=/ ".to_string()
        }
        _ => String::new(),
    }
}

/// Enable Debian multiarch (amd64 architecture in dpkg) and install the
/// :amd64 system libraries Tauri/WebKitGTK needs to cross-link x86_64
/// binaries from an ARM64 host. Idempotent — checks before installing.
fn ensure_linux_multiarch(profile: &VmProfile, session: &ssh2::Session) -> Result<()> {
    println!("→ Ensuring Linux multiarch (amd64) deps...");

    // Check sentinel package first; bail early if already installed.
    let check = ssh::exec(
        session,
        "dpkg-query -W -f='${Status}' libwebkit2gtk-4.1-dev:amd64 2>/dev/null | grep -c 'ok installed'",
    )
    .unwrap_or_default();
    if check.trim() == "1" {
        println!("  ✓ multiarch deps already installed");
        return Ok(());
    }

    // dpkg --add-architecture amd64 needs root; do everything via sudo in
    // one shell so apt update sees the new arch list.
    let cmd = "set -e; \
        sudo dpkg --add-architecture amd64; \
        sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq; \
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
          gcc-x86-64-linux-gnu \
          libwebkit2gtk-4.1-dev:amd64 \
          libgtk-3-dev:amd64 \
          libayatana-appindicator3-dev:amd64 \
          librsvg2-dev:amd64 \
          libssl-dev:amd64 \
          libxdo-dev:amd64 \
          libsoup-3.0-dev:amd64 \
          libjavascriptcoregtk-4.1-dev:amd64";

    let code = ssh::exec_streaming(profile, cmd)?;
    if code != 0 {
        bail!("multiarch setup failed (exit {code})");
    }
    println!("✓ multiarch deps installed");
    Ok(())
}

fn walk_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path  = entry.path();
        if path.is_dir() {
            out.extend(walk_files(&path)?);
        } else {
            out.push(path);
        }
    }
    Ok(out)
}
