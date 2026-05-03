//! `utm-dev vm run` — launch a built binary inside the VM and capture its
//! startup output. Linux uses Xvfb + openbox so GTK windows actually map;
//! Windows uses Start-Process with redirected stdout/stderr.
//!
//! Auto-detects the binary path from the project's Cargo.toml package name
//! when `--bin` is omitted. Tries `CARGO_TARGET_DIR/<triple>/release/<name>`
//! first, then Tauri default `src-tauri/target/...`, then plain Rust default.

use crate::vm::{profiles, ssh};

pub fn run(name: &str, bin: Option<&str>) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    ssh::check(profile)?;
    let session = ssh::connect(profile)?;

    let bin_owned;
    let bin = match bin {
        Some(b) => b,
        None => {
            bin_owned = auto_detect_bin(profile, &session)?;
            &bin_owned
        }
    };
    println!("→ binary: {bin}");

    println!("→ Launching {bin} in {name} (output → ~/.utm-dev-run/run.log)...");

    // Derive bin basename for `pkill <name>` — pkill -f matches the FULL
    // command line including our own shell's argv, so `pkill -f Xvfb`
    // kills the shell that's running pkill, severing the SSH connection
    // (exit 255). pkill by command name (no -f) matches /proc/N/comm only,
    // safe.
    let bin_name = std::path::Path::new(bin)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(bin);

    let cmd = match profile.os {
        profiles::GuestOs::Linux => format!(
            // We start Xvfb on a fixed DISPLAY=:99, then a tiny WM
            // (openbox) so windows actually get mapped, then the app.
            // Without a WM, bare Xvfb produces a black screenshot —
            // GTK windows open but aren't composited/mapped.
            //
            // setsid -f detaches from the SSH session's controlling terminal
            // so SIGHUP doesn't kill the children. We invoke this command
            // via `ssh` (no -tt) so the channel closes cleanly without
            // delivering signals to the detached descendants.
            //
            // pkill <name> (NOT -f) — `pkill -f` matches the FULL command
            // line including our own shell's argv, so `pkill -f Xvfb` kills
            // the shell itself (exit 255, SSH dies). Plain pkill matches
            // /proc/N/comm only, which is just the basename.
            "mkdir -p ~/.utm-dev-run; pkill Xvfb 2>/dev/null; pkill openbox 2>/dev/null; pkill {bin_name} 2>/dev/null; sleep 1; setsid -f Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >~/.utm-dev-run/xvfb.log 2>&1; sleep 1; DISPLAY=:99 setsid -f openbox --replace >~/.utm-dev-run/openbox.log 2>&1 || true; sleep 1; DISPLAY=:99 setsid -f '{bin}' >~/.utm-dev-run/run.log 2>&1; sleep 3; pgrep Xvfb >/dev/null && echo 'Xvfb running' || echo 'Xvfb DEAD'; pgrep {bin_name} >/dev/null && echo 'app running' || echo 'app DEAD — see ~/.utm-dev-run/run.log'; true"
        ),
        profiles::GuestOs::Windows => format!(
            // Start-Process detaches; redirect stdout/stderr to separate
            // files so we can also surface stderr in vm logs.
            //
            // Single line (no `^` continuation): cmd's `^<nl>` line-join
            // doesn't survive SSH delivery — the remote cmd sees literal
            // `^\n` and treats `-Command` as parameter-less, producing
            // PowerShell's help text. PowerShell's `;` works as a statement
            // separator inside the -Command string; that's all we need.
            r#"powershell -NoProfile -Command "$d='%USERPROFILE%\.utm-dev-run'; if (-not (Test-Path $d)) {{ New-Item -ItemType Directory -Path $d | Out-Null }}; $p = Start-Process -FilePath '{bin}' -RedirectStandardOutput ($d + '\\run.log') -RedirectStandardError ($d + '\\run.log.err') -PassThru; Write-Output ('PID=' + $p.Id)""#
        ),
    };

    // Bypass exec_streaming — it injects -tt on Linux which forces a pty;
    // pty session-close sends SIGHUP to backgrounded children even with
    // setsid+nohup, killing our Xvfb+app. We need *no* pty so the SSH
    // channel closes cleanly without disturbing detached processes.
    // libssh2's exec_with_exit also doesn't work here — it sends a kill
    // signal to the channel's pgid on close. So: invoke ssh directly,
    // no -tt, no -t.
    let target = format!("{}@localhost", profile.user);
    let port_str = profile.ssh_port.to_string();
    let status = std::process::Command::new("ssh")
        .args([
            "-p", &port_str,
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "LogLevel=ERROR",
            "-o", "BatchMode=yes",
        ])
        .arg(&target)
        .arg(&cmd)
        .status()?;
    if !status.success() {
        anyhow::bail!(
            "Failed to launch {bin} (exit {}). Check ~/.utm-dev-run/run.log on the VM",
            status.code().unwrap_or(-1)
        );
    }
    println!("✓ Launched. Tail output:  utm-dev vm logs --name {name} --kind run --follow");
    Ok(())
}

/// Read the project's `src-tauri/Cargo.toml` (or `Cargo.toml` fallback) for the
/// package name, derive the VM-side binary path. Tries common target-dir
/// locations on the VM and returns the first that exists. Tauri ARM64 Linux
/// builds default to `aarch64-unknown-linux-gnu`; Windows VMs always emit
/// `x86_64-pc-windows-msvc` (see GAPS #1).
fn auto_detect_bin(profile: &profiles::VmProfile, session: &ssh::Session) -> anyhow::Result<String> {
    let project_dir = std::env::current_dir()?;
    let project_name = project_dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("project dir has no name"))?
        .to_string_lossy()
        .to_string();

    let cargo_paths = [
        project_dir.join("src-tauri").join("Cargo.toml"),
        project_dir.join("Cargo.toml"),
    ];
    let cargo_content = cargo_paths.iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .ok_or_else(|| anyhow::anyhow!(
            "auto-detect failed: no Cargo.toml found in {} or src-tauri/. Pass --bin explicitly.",
            project_dir.display()
        ))?;
    let pkg_name = parse_package_name(&cargo_content)
        .ok_or_else(|| anyhow::anyhow!("auto-detect failed: no [package] name in Cargo.toml. Pass --bin explicitly."))?;

    let (triple, ext, sep) = match profile.os {
        profiles::GuestOs::Windows => ("x86_64-pc-windows-msvc", ".exe", '\\'),
        profiles::GuestOs::Linux   => ("aarch64-unknown-linux-gnu", "",   '/'),
    };
    let vm_home = match profile.os {
        profiles::GuestOs::Windows => format!("C:\\Users\\{}", profile.user),
        profiles::GuestOs::Linux   => format!("/home/{}", profile.user),
    };

    // Candidate paths in priority order:
    //   1. CARGO_TARGET_DIR/<triple>/release/<name>(.exe) — wins if env set on VM
    //   2. <vm_project>/src-tauri/target/<triple>/release/<name>(.exe) — Tauri default
    //   3. <vm_project>/target/<triple>/release/<name>(.exe) — non-Tauri Rust default
    let probe = if profile.os == profiles::GuestOs::Windows {
        r#"echo BEGIN_CTD & if defined CARGO_TARGET_DIR (echo %CARGO_TARGET_DIR%) else (echo DEFAULT) & echo END_CTD"#.to_string()
    } else {
        r#"echo BEGIN_CTD; echo "${CARGO_TARGET_DIR:-DEFAULT}"; echo END_CTD"#.to_string()
    };
    let (probe_out, _) = ssh::exec_with_exit(session, &probe)?;
    let ctd = probe_out
        .lines()
        .skip_while(|l| l.trim() != "BEGIN_CTD")
        .nth(1)
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| "DEFAULT".into());

    let mut candidates: Vec<String> = Vec::new();
    if ctd != "DEFAULT" && !ctd.is_empty() {
        candidates.push(format!("{ctd}{sep}{triple}{sep}release{sep}{pkg_name}{ext}"));
    }
    candidates.push(format!(
        "{vm_home}{sep}{project_name}{sep}src-tauri{sep}target{sep}{triple}{sep}release{sep}{pkg_name}{ext}"
    ));
    candidates.push(format!(
        "{vm_home}{sep}{project_name}{sep}target{sep}{triple}{sep}release{sep}{pkg_name}{ext}"
    ));

    for cand in &candidates {
        let test_cmd = if profile.os == profiles::GuestOs::Windows {
            format!(r#"if exist "{cand}" (echo FOUND) else (echo NOPE)"#)
        } else {
            format!(r#"[ -x "{cand}" ] && echo FOUND || echo NOPE"#)
        };
        let out = ssh::exec(session, &test_cmd).unwrap_or_default();
        if out.contains("FOUND") {
            return Ok(cand.clone());
        }
    }

    anyhow::bail!(
        "auto-detect failed: '{pkg_name}{ext}' not found in any of:\n  - {}\n\
         Run `utm-dev vm build` first, or pass --bin explicitly.",
        candidates.join("\n  - ")
    );
}

/// Tiny TOML scan for `[package] ... name = "x"`. Avoids pulling in a real
/// TOML parser for one field — same approach as import::rewrite_plist_name.
fn parse_package_name(content: &str) -> Option<String> {
    let pkg_idx = content.find("[package]")?;
    for line in content[pkg_idx..].lines().skip(1) {
        let l = line.trim();
        if l.starts_with('[') { return None; } // entered next section
        if let Some(rest) = l.strip_prefix("name") {
            let rest = rest.trim_start_matches([' ', '\t', '=']);
            let rest = rest.trim();
            if let Some(stripped) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                return Some(stripped.to_string());
            }
        }
    }
    None
}
