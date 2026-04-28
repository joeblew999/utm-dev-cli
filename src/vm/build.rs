/// Sync project to VM, run `mise run build`, pull artifacts back.
/// Ported from vm/build.ts.
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::profiles::{GuestOs, VmProfile};
use super::ssh;

pub fn run(profile: &VmProfile, project_dir: &Path) -> Result<()> {
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
    let session = ssh::connect(profile)?;

    // ── Sync code ─────────────────────────────────────────────────────────────

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
    println!("✓ Code synced");

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

    // On Windows, source VsDevCmd.bat to put link.exe + MSVC headers/libs
    // on PATH/INCLUDE/LIB. cargo's vswhere-based detection isn't reliable
    // on Vagrant's utm/windows-11 box (vswhere returns empty), so we wire
    // the env in directly. -arch=arm64 -host_arch=arm64 because the VM is
    // ARM64 building native targets.
    let vsdevcmd = r#"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat"#;
    let win_msvc = format!(r#"call "{vsdevcmd}" -arch=arm64 -host_arch=arm64 -no_logo"#);

    println!("→ Running mise install in VM (persistent log at {log_path_h})...");
    let install_cmd = if profile.os == GuestOs::Windows {
        format!(
            r#"{mkdir_log} & cd /d "{vm_project_dir}" && ({win_msvc} && {mise} trust --yes && {mise} install) >> "{log_path_h}" 2>&1"#
        )
    } else {
        format!(r#"{mkdir_log} && {linux_tee}cd "{vm_project_dir}" && {mise} trust --yes && {mise} install"#)
    };
    let code = ssh::exec_streaming(profile, &install_cmd)?;
    if code != 0 { bail!("mise install failed inside VM (exit {code}) — see: utm-dev vm logs --name {}", profile.name); }
    println!("✓ Tools installed");

    // ── Build ─────────────────────────────────────────────────────────────────

    let platform_label = match profile.os { GuestOs::Windows => "Windows", GuestOs::Linux => "Linux" };
    println!("→ Building Tauri {platform_label} app (first run may take 10–30 min — tail with: vm logs --name {} --follow)...", profile.name);

    let build_cmd = if profile.os == GuestOs::Windows {
        format!(
            r#"cd /d "{vm_project_dir}" && ({win_msvc} && {mise} run build) >> "{log_path_h}" 2>&1"#
        )
    } else {
        format!(r#"{linux_tee}cd "{vm_project_dir}" && {mise} run build"#)
    };
    let code = ssh::exec_streaming(profile, &build_cmd)?;
    if code != 0 { bail!("Build failed inside VM (exit {code}) — see: utm-dev vm logs --name {}", profile.name); }
    println!("✓ Build complete");

    // ── Pull artifacts ────────────────────────────────────────────────────────

    println!("→ Pulling artifacts...");
    std::fs::create_dir_all(&artifacts_dir)?;

    // Resolve the bundle path. cargo respects CARGO_TARGET_DIR which we may
    // have set on the VM (e.g. Windows uses D:\target to keep C: from filling
    // up). Default is `src-tauri/target` relative to the project.
    let probe = if profile.os == GuestOs::Windows {
        // Returns CARGO_TARGET_DIR on stdout if set, else "DEFAULT"
        r#"if defined CARGO_TARGET_DIR (echo %CARGO_TARGET_DIR%) else (echo DEFAULT)"#.to_string()
    } else {
        r#"echo "${CARGO_TARGET_DIR:-DEFAULT}""#.to_string()
    };
    let (probe_out, _) = ssh::exec_with_exit(&session, &probe)?;
    let target_dir = probe_out.trim().lines().last().unwrap_or("DEFAULT").trim();
    let bundle_path = if target_dir == "DEFAULT" || target_dir.is_empty() {
        format!("{vm_project_dir}{sep}src-tauri{sep}target{sep}release{sep}bundle")
    } else {
        format!("{target_dir}{sep}release{sep}bundle")
    };
    println!("  bundle path: {bundle_path}");

    // Archive on VM
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
    if code != 0 { bail!("Failed to archive artifacts on VM:\n{out}"); }

    // Download (same Linux-absolute / Windows-relative split as the upload path)
    let remote_artifacts = match profile.os {
        GuestOs::Linux   => format!("/home/{}/artifacts.tar.gz", profile.user),
        GuestOs::Windows => "artifacts.tar.gz".to_string(),
    };
    let local_tar = artifacts_dir.join("artifacts.tar.gz");
    ssh::download(profile, &remote_artifacts, &local_tar)?;

    // Clean up on VM
    let _ = if profile.os == GuestOs::Linux {
        ssh::exec(&session, "rm ~/artifacts.tar.gz")
    } else {
        ssh::exec(&session, r#"del "%USERPROFILE%\artifacts.tar.gz""#)
    };

    // Extract locally
    let status = Command::new("tar")
        .args([
            "-xzf",
            local_tar.to_str().unwrap(),
            "-C",
            artifacts_dir.to_str().unwrap(),
        ])
        .status()
        .context("extracting artifacts")?;
    if !status.success() {
        bail!("Failed to extract artifacts locally");
    }
    let _ = std::fs::remove_file(&local_tar);

    // List what we got
    println!("✓ Artifacts in {}:", artifacts_dir.display());
    let exts: &[&str] = match profile.os {
        GuestOs::Linux   => &["deb", "AppImage", "rpm"],
        GuestOs::Windows => &["msi", "exe"],
    };
    for path in walk_files(&artifacts_dir)? {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if exts.contains(&ext) {
            let size = std::fs::metadata(&path)?.len();
            println!("  {} ({:.1} MB)", path.display(), size as f64 / 1_048_576.0);
        }
    }

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
