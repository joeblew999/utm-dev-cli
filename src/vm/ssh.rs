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
    let status = std::process::Command::new("ssh")
        .args([
            "-p", &profile.ssh_port.to_string(),
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "LogLevel=ERROR",
            "-o", "BatchMode=yes",
        ])
        .arg(&target)
        .arg(cmd)
        .status()
        .context("spawning ssh subprocess")?;
    Ok(status.code().unwrap_or(1))
}

#[allow(dead_code)]
/// Upload a local file to the VM via SCP.
pub fn upload(session: &Session, local: &std::path::Path, remote_path: &str) -> Result<()> {
    let data = std::fs::read(local).with_context(|| format!("reading {}", local.display()))?;
    let mut channel = session
        .scp_send(
            std::path::Path::new(remote_path),
            0o644,
            data.len() as u64,
            None,
        )
        .with_context(|| format!("SCP send to {remote_path}"))?;
    use std::io::Write;
    channel.write_all(&data).context("writing SCP data")?;
    channel.send_eof().context("SCP send EOF")?;
    channel.wait_eof().context("SCP wait EOF")?;
    channel.close().context("SCP close")?;
    channel.wait_close().context("SCP wait close")?;
    Ok(())
}

/// Download a remote file to a local path via SCP.
pub fn download(session: &Session, remote_path: &str, local: &std::path::Path) -> Result<()> {
    let (mut channel, _stat) = session
        .scp_recv(std::path::Path::new(remote_path))
        .with_context(|| format!("SCP recv {remote_path}"))?;
    let mut data = Vec::new();
    channel.read_to_end(&mut data).context("reading SCP data")?;
    channel.send_eof().context("SCP send EOF")?;
    channel.wait_eof().context("SCP wait EOF")?;
    channel.close().context("SCP close")?;
    channel.wait_close().context("SCP wait close")?;
    std::fs::write(local, &data)
        .with_context(|| format!("writing {}", local.display()))
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
