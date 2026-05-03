//! SSH helpers — all operations shell out to the host's `ssh` / `scp` CLI.
//!
//! Why not libssh2 (the `ssh2` crate)?
//!   - On Windows targets, ssh2 + `vendored-openssl` builds OpenSSL from
//!     source via openssl-src, which requires perl on the build VM. We
//!     don't want perl on a build VM.
//!   - Every host that runs utm-dev (macOS Apple Silicon) ships OpenSSH.
//!     Every Vagrant box utm-dev imports also ships OpenSSH.
//!   - We already shelled out to `ssh` for `exec_streaming` (libssh2's
//!     read_to_string blocks long compiles). One transport everywhere.
//!
//! `Session` is a cheap handle — no persistent connection. Each operation
//! spawns its own ssh subprocess. The host's ssh-agent and `~/.ssh/id_*`
//! provide auth, same as the existing `exec_streaming` path. `BatchMode=yes`
//! ensures we never block waiting for an interactive password prompt.
use anyhow::{Context, Result};
use std::process::Command;

use super::profiles::{GuestOs, VmProfile};

/// Cheap connection handle. Owns a `VmProfile` clone so it lives independently
/// of whatever loaned it the profile. No TCP/SSH state inside — every call
/// spawns its own `ssh` subprocess.
#[derive(Clone)]
pub struct Session {
    profile: VmProfile,
}

/// "Connect" by running an `ssh ... echo ok` health check. Returns a
/// `Session` if the host reaches the VM and auth succeeds. No persistent
/// connection is held — the Session is just a typed bag of profile info.
pub fn connect(profile: &VmProfile) -> Result<Session> {
    let sess = Session {
        profile: profile.clone(),
    };
    let (out, code) = exec_with_exit(&sess, "echo ok")?;
    if code != 0 || !out.contains("ok") {
        anyhow::bail!(
            "SSH not reachable on port {} (user {}). Run: utm-dev vm up --name {}\n  ssh said: {}",
            profile.ssh_port,
            profile.user,
            profile.name,
            out
        );
    }
    Ok(sess)
}

/// Reachability check — used by callers that want a clear error before
/// doing further work. Same as `connect()` but discards the handle.
pub fn check(profile: &VmProfile) -> Result<()> {
    connect(profile).map(|_| ())
}

/// Run a command and return its stdout+stderr as a single trimmed string.
pub fn exec(session: &Session, cmd: &str) -> Result<String> {
    let (out, _) = exec_with_exit(session, cmd)?;
    Ok(out)
}

/// Run a command and return (combined stdout+stderr, exit code).
///
/// Output bytes are converted with `from_utf8_lossy` because Windows tools
/// (DISM, Get-Content, mise console) emit in the local codepage or UTF-16,
/// neither of which is valid UTF-8. Lossy conversion replaces invalid bytes
/// with U+FFFD; strict from_utf8 would bail and the caller sees nothing.
pub fn exec_with_exit(session: &Session, cmd: &str) -> Result<(String, i32)> {
    let p = &session.profile;
    let target = format!("{}@localhost", p.user);
    let port_str = p.ssh_port.to_string();
    let output = Command::new("ssh")
        .args([
            "-p",
            &port_str,
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "LogLevel=ERROR",
            "-o",
            "BatchMode=yes",
        ])
        .arg(&target)
        .arg(cmd)
        .output()
        .with_context(|| format!("spawning ssh subprocess for: {cmd}"))?;

    // Combine stdout + stderr — many tools (mise, dism, cargo) write progress
    // to stderr but errors we care about. Callers can't inspect them
    // separately anyway with this API.
    let mut combined = output.stdout;
    if !output.stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with(b"\n") {
            combined.push(b'\n');
        }
        combined.extend_from_slice(&output.stderr);
    }
    let text = String::from_utf8_lossy(&combined).trim().to_string();
    let code = output.status.code().unwrap_or(1);
    Ok((text, code))
}

/// Run a PowerShell script on a Windows guest via the existing ssh transport.
/// Encodes the script as UTF-16LE+Base64 (so braces, quotes, and unicode
/// survive shell quoting) and strips PowerShell's CLIXML envelope from the
/// combined output — that XML blob is purely an artifact of PowerShell
/// detecting a non-interactive ssh and folding info/progress streams onto
/// stderr; humans never want to see it.
pub fn exec_ps_windows(session: &Session, script: &str) -> Result<(String, i32)> {
    use base64::Engine;
    let utf16: Vec<u8> = script
        .encode_utf16()
        .flat_map(|c| c.to_le_bytes())
        .collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&utf16);
    let cmd = format!("powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand {encoded}");
    let (out, code) = exec_with_exit(session, &cmd)?;
    Ok((strip_clixml(&out), code))
}

/// Strip PowerShell's CLIXML envelope (a single `#< CLIXML\n<Objs ...>...</Objs>`
/// block) from combined stdout/stderr. The envelope only ever appears once
/// per invocation and contains nothing a human reader cares about, so we cut
/// from the marker to end-of-string.
fn strip_clixml(s: &str) -> String {
    match s.find("#< CLIXML") {
        Some(i) => s[..i].trim_end().to_string(),
        None => s.to_string(),
    }
}

/// Run a command via the `ssh` CLI subprocess so stdout/stderr stream live
/// to the user's terminal — `output()` blocks until completion, which makes
/// long ops like `cargo build` go silent for 10+ minutes. Returns the exit
/// code.
pub fn exec_streaming(profile: &VmProfile, cmd: &str) -> Result<i32> {
    let target = format!("{}@localhost", profile.user);
    // -tt forces a pseudo-TTY which keeps remote stdout line-buffered on
    // Linux (cargo/mise pipe-detect and buffer otherwise — silent 10-min
    // compiles). On Windows cmd.exe, -tt corrupts the session: the cmd
    // exits immediately without running anything, returning 0. So Linux
    // gets -tt; Windows uses plain pipes (we redirect to a log file at
    // the cmd-level for visibility instead).
    let port_str = profile.ssh_port.to_string();
    let mut args: Vec<&str> = vec![
        "-p",
        &port_str,
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        "-o",
        "LogLevel=ERROR",
        "-o",
        "BatchMode=yes",
    ];
    if profile.os == GuestOs::Linux {
        args.insert(0, "-tt");
    }
    let status = Command::new("ssh")
        .args(&args)
        .arg(&target)
        .arg(cmd)
        .status()
        .context("spawning ssh subprocess")?;
    Ok(status.code().unwrap_or(1))
}

/// Upload a local file to the VM via `scp`.
pub fn upload(profile: &VmProfile, local: &std::path::Path, remote_path: &str) -> Result<()> {
    scp(
        profile,
        local.to_str().context("local path not UTF-8")?,
        &format!("{}@localhost:{remote_path}", profile.user),
    )
}

/// Download a remote file to the local host via `scp`.
pub fn download(profile: &VmProfile, remote_path: &str, local: &std::path::Path) -> Result<()> {
    scp(
        profile,
        &format!("{}@localhost:{remote_path}", profile.user),
        local.to_str().context("local path not UTF-8")?,
    )
}

fn scp(profile: &VmProfile, src: &str, dst: &str) -> Result<()> {
    let status = Command::new("scp")
        .args([
            "-P",
            &profile.ssh_port.to_string(),
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "LogLevel=ERROR",
            "-o",
            "BatchMode=yes",
            src,
            dst,
        ])
        .status()
        .context("spawning scp")?;
    if !status.success() {
        anyhow::bail!("scp {} -> {} exited {}", src, dst, status);
    }
    Ok(())
}
