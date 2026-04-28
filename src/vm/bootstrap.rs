/// First-run bootstrap: installs build tools, Rust, and mise in a fresh VM.
/// Idempotent — safe to run multiple times (checks before installing).
use anyhow::{bail, Context, Result};

use super::profiles::{BootstrapMode, GuestOs, VmProfile};
use super::{ssh, winrm};

pub fn run(profile: &VmProfile, session: &ssh2::Session) -> Result<()> {
    if profile.bootstrap == BootstrapMode::None {
        return Ok(());
    }
    match profile.os {
        GuestOs::Linux   => linux(session, profile),
        GuestOs::Windows => windows(profile),
    }
}

// ── Linux bootstrap ───────────────────────────────────────────────────────────

fn linux(session: &ssh2::Session, profile: &VmProfile) -> Result<()> {
    println!("→ Bootstrapping Linux VM (mode: {:?})...", profile.bootstrap);

    if profile.bootstrap == BootstrapMode::SshOnly {
        let out = ssh::exec(session, "echo ok")?;
        if out.contains("ok") {
            println!("✓ SSH verified");
        }
        return Ok(());
    }

    // Full bootstrap — check before each step (idempotent)

    // Step 1: build-essential + curl + git
    let installed = ssh::exec(session,
        "dpkg -s build-essential 2>/dev/null | grep -c 'ok installed'"
    ).unwrap_or_default();
    if installed.trim() != "1" {
        run_step(session, "update packages",
            "sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq")?;
        run_step(session, "install build deps",
            "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
             build-essential curl git pkg-config")?;
    } else {
        println!("  ✓ build-essential already installed");
    }

    // Step 2: Tauri Linux dependencies
    let webkit = ssh::exec(session,
        "dpkg -s libwebkit2gtk-4.1-dev 2>/dev/null | grep -c 'ok installed'"
    ).unwrap_or_default();
    if webkit.trim() != "1" {
        run_step(session, "install Tauri Linux deps",
            "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
             libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
             librsvg2-dev libssl-dev libxdo-dev patchelf wget file \
             libsoup-3.0-dev libjavascriptcoregtk-4.1-dev")?;
    } else {
        println!("  ✓ Tauri deps already installed");
    }

    // Step 3: mise
    let mise = ssh::exec(session,
        "~/.local/bin/mise --version 2>/dev/null || mise --version 2>/dev/null || echo missing"
    ).unwrap_or_default();
    if mise.contains("missing") || mise.is_empty() {
        run_step(session, "install mise", "curl https://mise.run | sh")?;
    } else {
        println!("  ✓ mise already installed ({})", mise.trim());
    }
    run_step(session, "activate mise in .bashrc",
        r#"grep -q 'mise activate' ~/.bashrc || echo 'eval "$(~/.local/bin/mise activate bash)"' >> ~/.bashrc"#)?;

    // Step 4: Rust via mise
    let rust = ssh::exec(session,
        "~/.cargo/bin/rustc --version 2>/dev/null || rustc --version 2>/dev/null || echo missing"
    ).unwrap_or_default();
    if rust.contains("missing") || rust.is_empty() {
        run_step(session, "install Rust", "~/.local/bin/mise use --global rust@stable")?;
    } else {
        println!("  ✓ Rust already installed ({})", rust.trim());
    }

    // Step 5: linux-dev extras (Debian 12 with GNOME)
    if profile.name == "linux-dev" {
        let xdg = ssh::exec(session,
            "dpkg -s xdg-utils 2>/dev/null | grep -c 'ok installed'"
        ).unwrap_or_default();
        if xdg.trim() != "1" {
            run_step(session, "install desktop extras",
                "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
                 xdg-utils fonts-noto-color-emoji")?;
        }
    }

    println!("✓ Linux bootstrap complete");
    Ok(())
}

fn run_step(session: &ssh2::Session, label: &str, cmd: &str) -> Result<()> {
    print!("  {label}...");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let (out, code) = ssh::exec_with_exit(session, cmd)?;
    if code != 0 {
        println!(" ✗ (exit {code})");
        eprintln!("    {out}");
    } else {
        println!(" ✓");
    }
    Ok(())
}

// ── Windows bootstrap ─────────────────────────────────────────────────────────

fn windows(profile: &VmProfile) -> Result<()> {
    let port = profile
        .winrm_port
        .ok_or_else(|| anyhow::anyhow!("No WinRM port configured for '{}'", profile.name))?;

    let w = winrm::WinRM::new("127.0.0.1", port, profile.user, profile.pass)?;
    if !w.ping() {
        bail!(
            "WinRM not reachable on port {port} — is the VM running and WinRM enabled?\n\
             To enable WinRM manually, run in the VM: winrm quickconfig -force"
        );
    }

    println!("→ Bootstrapping Windows VM via WinRM (port {port})...");

    // Step 1: OpenSSH Server
    let check = w.run_ps(
        "(Get-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0).State",
    )?;
    if !check.stdout.contains("Installed") {
        println!("  Installing OpenSSH Server (~3 min, downloads from Windows Update)...");
        w.run_elevated(
            "Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0",
            360,
        )?;
        println!("  ✓ OpenSSH Server installed");
    } else {
        println!("  ✓ OpenSSH Server already installed");
    }

    // Step 2: Start + enable sshd
    w.run_ps("Start-Service sshd -ErrorAction SilentlyContinue; Set-Service sshd -StartupType Automatic")?;
    println!("  ✓ sshd running");

    // Step 3: Authorise the host's public key so key-based SSH auth works
    let pub_key = find_public_key()?;
    let ps = format!(
        r"
$key = '{pub_key}'
$dir = 'C:\ProgramData\ssh'
if (-not (Test-Path $dir)) {{ New-Item -ItemType Directory -Path $dir | Out-Null }}
Set-Content '$dir\administrators_authorized_keys' $key -Encoding ASCII
icacls '$dir\administrators_authorized_keys' /inheritance:r /grant 'Administrators:F' /grant 'SYSTEM:F' | Out-Null
"
    );
    w.run_ps(&ps)?;
    println!("  ✓ SSH authorized key installed");

    // Step 4: Fix sshd_config (clean minimal — no invalid Match blocks)
    let sshd_cfg = r"
$cfg = @'
Port 22
PubkeyAuthentication yes
AuthorizedKeysFile .ssh/authorized_keys
PasswordAuthentication yes
Subsystem sftp sftp-server.exe
'@
Set-Content 'C:\ProgramData\ssh\sshd_config' $cfg -Encoding ASCII
Restart-Service sshd
";
    w.run_ps(sshd_cfg)?;
    println!("  ✓ sshd_config updated");

    // Step 5: Enable LocalAccountTokenFilterPolicy (so WinRM works with local admin)
    w.run_ps(
        "Set-ItemProperty -Path 'HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System' \
         -Name LocalAccountTokenFilterPolicy -Value 1 -Type DWord -Force",
    )?;
    println!("  ✓ LocalAccountTokenFilterPolicy = 1");

    // Step 6: mise for Windows
    let check_mise = w.run_ps(
        "Get-Command mise -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name",
    )?;
    if check_mise.stdout.trim().is_empty() {
        println!("  Installing mise...");
        w.run_elevated("irm https://mise.run/install.ps1 | iex", 180)?;
        println!("  ✓ mise installed");
    } else {
        println!("  ✓ mise already installed");
    }

    println!("✓ Windows bootstrap complete");
    Ok(())
}

fn find_public_key() -> Result<String> {
    let home = dirs::home_dir().context("no home dir")?;
    for name in &["id_ed25519.pub", "id_rsa.pub", "id_ecdsa.pub"] {
        let path = home.join(".ssh").join(name);
        if path.exists() {
            return std::fs::read_to_string(&path)
                .map(|s| s.trim().to_string())
                .with_context(|| format!("reading {}", path.display()));
        }
    }
    bail!("No SSH public key found in ~/.ssh/ — generate one with: ssh-keygen -t ed25519")
}

