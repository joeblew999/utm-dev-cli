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

    let status = Command::new("tar")
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

    let remote_tar = match profile.os {
        GuestOs::Linux   => format!("/home/{}/sync.tar.gz", profile.user),
        GuestOs::Windows => format!("C:/Users/{}/sync.tar.gz", profile.user),
    };
    ssh::upload(&session, &tmp_tar, &remote_tar)?;
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
        let (out, code) = ssh::exec_with_exit(
            &session,
            &format!(
                r#"if not exist "{vm_project_dir}" mkdir "{vm_project_dir}" && cd "{vm_project_dir}" && tar -xzf "%USERPROFILE%\sync.tar.gz" && del "%USERPROFILE%\sync.tar.gz""#
            ),
        )?;
        if code != 0 { bail!("untar failed on VM:\n{out}"); }
    }
    println!("✓ Code synced");

    // ── Install tools ─────────────────────────────────────────────────────────

    println!("→ Running mise install in VM (live output)...");
    let mise  = if profile.os == GuestOs::Linux { "~/.local/bin/mise" } else { "mise" };
    let code = ssh::exec_streaming(
        profile,
        &format!(r#"cd "{vm_project_dir}" && {mise} trust --yes && {mise} install"#),
    )?;
    if code != 0 { bail!("mise install failed inside VM (exit {code})"); }
    println!("✓ Tools installed");

    // ── Build ─────────────────────────────────────────────────────────────────

    let platform_label = match profile.os { GuestOs::Windows => "Windows", GuestOs::Linux => "Linux" };
    println!("→ Building Tauri {platform_label} app (live output; first run may take 10–30 min)...");

    let code = ssh::exec_streaming(
        profile,
        &format!(r#"cd "{vm_project_dir}" && {mise} run build"#),
    )?;
    if code != 0 { bail!("Build failed inside VM (exit {code})"); }
    println!("✓ Build complete");

    // ── Pull artifacts ────────────────────────────────────────────────────────

    println!("→ Pulling artifacts...");
    std::fs::create_dir_all(&artifacts_dir)?;

    let bundle_path = format!(
        "{vm_project_dir}{sep}src-tauri{sep}target{sep}release{sep}bundle"
    );

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

    // Download
    let remote_artifacts = match profile.os {
        GuestOs::Linux   => format!("/home/{}/artifacts.tar.gz", profile.user),
        GuestOs::Windows => format!("C:/Users/{}/artifacts.tar.gz", profile.user),
    };
    let local_tar = artifacts_dir.join("artifacts.tar.gz");
    ssh::download(&session, &remote_artifacts, &local_tar)?;

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
