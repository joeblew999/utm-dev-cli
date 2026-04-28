/// SSH helpers using libssh2 — replaces the sshpass subprocess approach from TypeScript.
use anyhow::{Context, Result};
use ssh2::Session;
use std::io::Read;
use std::net::TcpStream;
use std::time::Duration;

use super::profiles::VmProfile;

pub fn connect(profile: &VmProfile) -> Result<Session> {
    let addr = format!("127.0.0.1:{}", profile.ssh_port);
    let tcp = TcpStream::connect_timeout(
        &addr.parse().context("parsing SSH addr")?,
        Duration::from_secs(5),
    )
    .with_context(|| format!("TCP connect to {addr}"))?;

    let mut sess = Session::new().context("creating SSH session")?;
    sess.set_tcp_stream(tcp);
    sess.handshake().context("SSH handshake")?;

    // Try auth methods in order: agent → key files → password
    if try_agent(&mut sess, profile.user).is_ok() {
        return Ok(sess);
    }
    if try_key_files(&mut sess, profile.user).is_ok() {
        return Ok(sess);
    }
    if !profile.pass.is_empty() {
        sess.userauth_password(profile.user, profile.pass)
            .context("SSH password auth")?;
        return Ok(sess);
    }

    anyhow::bail!(
        "All SSH auth methods failed for {}@127.0.0.1:{}",
        profile.user,
        profile.ssh_port
    )
}

fn try_agent(sess: &mut Session, user: &str) -> Result<()> {
    let mut agent = sess.agent().context("opening SSH agent")?;
    agent.connect().context("connecting to SSH agent")?;
    agent.list_identities().context("listing agent identities")?;
    for identity in agent.identities()? {
        if agent.userauth(user, &identity).is_ok() && sess.authenticated() {
            return Ok(());
        }
    }
    anyhow::bail!("agent auth failed")
}

fn try_key_files(sess: &mut Session, user: &str) -> Result<()> {
    let home = dirs::home_dir().context("no home dir")?;
    let candidates = [
        home.join(".ssh").join("id_ed25519"),
        home.join(".ssh").join("id_rsa"),
        home.join(".ssh").join("id_ecdsa"),
    ];
    for privkey in &candidates {
        if !privkey.exists() {
            continue;
        }
        let pubkey = privkey.with_extension("pub");
        let pub_opt = if pubkey.exists() { Some(pubkey.as_path()) } else { None };
        if sess
            .userauth_pubkey_file(user, pub_opt, privkey, None)
            .is_ok()
            && sess.authenticated()
        {
            return Ok(());
        }
    }
    anyhow::bail!("key file auth failed")
}

pub fn exec(session: &Session, cmd: &str) -> Result<String> {
    let mut channel = session.channel_session().context("opening SSH channel")?;
    channel.exec(cmd).with_context(|| format!("exec: {cmd}"))?;
    let mut output = String::new();
    channel.read_to_string(&mut output).context("reading SSH output")?;
    channel.wait_close().context("waiting for channel close")?;
    Ok(output.trim().to_string())
}

pub fn exec_with_exit(session: &Session, cmd: &str) -> Result<(String, i32)> {
    let mut channel = session.channel_session().context("opening SSH channel")?;
    channel.exec(cmd).with_context(|| format!("exec: {cmd}"))?;
    let mut output = String::new();
    channel.read_to_string(&mut output).context("reading SSH output")?;
    channel.wait_close().context("waiting for channel close")?;
    let exit_code = channel.exit_status().unwrap_or(1);
    Ok((output.trim().to_string(), exit_code))
}

/// Run a command via the `ssh` CLI subprocess so stdout/stderr stream live
/// to the user's terminal (libssh2's read_to_string blocks until completion,
/// which makes long ops like `cargo build` go silent for 10+ minutes).
/// Returns the exit code.
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
        "-p", &port_str,
        "-o", "StrictHostKeyChecking=no",
        "-o", "UserKnownHostsFile=/dev/null",
        "-o", "LogLevel=ERROR",
        "-o", "BatchMode=yes",
    ];
    if profile.os == super::profiles::GuestOs::Linux {
        args.insert(0, "-tt");
    }
    let status = std::process::Command::new("ssh")
        .args(&args)
        .arg(&target)
        .arg(cmd)
        .status()
        .context("spawning ssh subprocess")?;
    Ok(status.code().unwrap_or(1))
}

/// Upload a local file to the VM. Shells out to `scp` because libssh2's
/// scp_send is unreliable against Windows OpenSSH (relative dest paths
/// silently no-op).
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
    let status = std::process::Command::new("scp")
        .args([
            "-P", &profile.ssh_port.to_string(),
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "LogLevel=ERROR",
            "-o", "BatchMode=yes",
            src, dst,
        ])
        .status()
        .context("spawning scp")?;
    if !status.success() {
        anyhow::bail!("scp {} -> {} exited {}", src, dst, status);
    }
    Ok(())
}

/// Check SSH is reachable — exit 1 with a helpful message if not.
pub fn check(profile: &VmProfile) -> Result<()> {
    let sess = connect(profile).with_context(|| {
        format!(
            "Cannot connect via SSH on port {}. Run: utm-dev vm up --name {}",
            profile.ssh_port, profile.name
        )
    })?;
    let out = exec(&sess, "echo ok")?;
    if !out.contains("ok") {
        anyhow::bail!(
            "SSH connected but echo test failed on port {}",
            profile.ssh_port
        );
    }
    Ok(())
}
