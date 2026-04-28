/// First-run bootstrap: installs build tools, Rust, and mise in a fresh VM.
/// Idempotent — safe to run multiple times (checks before installing).
use anyhow::{bail, Context, Result};

use super::profiles::{BootstrapMode, GuestOs, VmProfile};
use super::{ssh, winrm};

pub fn run(profile: &VmProfile) -> Result<()> {
    if profile.bootstrap == BootstrapMode::None {
        return Ok(());
    }
    match profile.os {
        GuestOs::Linux   => {
            // Linux is reachable via SSH right after wait_for_boot succeeds.
            let session = ssh::connect(profile)?;
            linux(&session, profile)
        }
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
    let sshd_state = w.run_ps(
        "Get-Service sshd -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Status",
    )?;
    if !sshd_state.stdout.trim().eq_ignore_ascii_case("Running") {
        let cap = w.run_ps(
            "(Get-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0).State",
        )?;
        if !cap.stdout.contains("Installed") {
            println!("  Installing OpenSSH Server (~3 min, downloads from Windows Update)...");
            w.run_elevated(
                "Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0",
                360,
            )?;
        }
        // Configure sshd_config: uncomment PasswordAuthentication, comment out the
        // Match Group administrators block so ~/.ssh/authorized_keys works for all users.
        // Also keep __PROGRAMDATA__ path working for admin accounts.
        w.run_elevated(r"
$p = 'C:\ProgramData\ssh\sshd_config'
$c = Get-Content $p
$o = @()
foreach ($l in $c) {
    if ($l -match '^#PasswordAuthentication yes')              { $o += 'PasswordAuthentication yes' }
    elseif ($l -match '^Match Group administrators')           { $o += '#Match Group administrators' }
    elseif ($l -match 'AuthorizedKeysFile __PROGRAMDATA__')   { $o += '#AuthorizedKeysFile __PROGRAMDATA__/ssh/administrators_authorized_keys' }
    else                                                       { $o += $l }
}
$o | Set-Content $p -Force
Start-Service sshd -ErrorAction SilentlyContinue
Set-Service -Name sshd -StartupType Automatic
Restart-Service sshd
", 60)?;
        println!("  ✓ OpenSSH Server installed and configured");
    } else {
        println!("  ✓ sshd already running");
    }

    // Step 2: Authorise the host's public key (~/.ssh/authorized_keys — matches sshd_config)
    let pub_key = find_public_key()?;
    let key_ps = format!(
        r#"
$key = '{pub_key}'
$dir = "$env:USERPROFILE\.ssh"
if (-not (Test-Path $dir)) {{ New-Item -ItemType Directory -Path $dir | Out-Null }}
$f = "$dir\authorized_keys"
if (-not (Test-Path $f) -or ((Get-Content $f) -notcontains $key)) {{
    Add-Content $f $key -Encoding ASCII
}}
icacls $f /inheritance:r /grant ($env:USERNAME + ':F') /grant 'SYSTEM:F' | Out-Null
"#
    );
    w.run_ps(&key_ps)?;
    println!("  ✓ SSH authorized key installed");

    // Step 3: LocalAccountTokenFilterPolicy — lets WinRM work with local admin accounts
    w.run_ps(
        "Set-ItemProperty -Path 'HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System' \
         -Name LocalAccountTokenFilterPolicy -Value 1 -Type DWord -Force",
    )?;
    println!("  ✓ LocalAccountTokenFilterPolicy = 1");

    // ssh-only mode stops here (windows-test profile)
    if profile.bootstrap == BootstrapMode::SshOnly {
        println!("✓ Windows bootstrap complete (SSH only)");
        return Ok(());
    }

    // ── Full mode: dev tools ────────────────────────────────────────────────

    // Step 4: VS Build Tools with C++ workload (needed by Rust/MSVC on Windows)
    let vswhere = r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe";
    let vc_check = w.run_ps(&format!(
        r#"if (Test-Path '{vswhere}') {{ & '{vswhere}' -products * -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null }} else {{ '' }}"#
    ))?;
    if vc_check.stdout.trim().is_empty() {
        println!("  Downloading VS Build Tools bootstrapper...");
        w.run_ps(r"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Invoke-WebRequest -Uri 'https://aka.ms/vs/17/release/vs_buildtools.exe' -OutFile 'C:\vs_buildtools.exe' -UseBasicParsing
")?;
        println!("  Installing VS Build Tools + C++ workload (10-15 min on ARM64)...");
        w.run_elevated(r"
$p = Start-Process -FilePath 'C:\vs_buildtools.exe' -ArgumentList @(
    '--add', 'Microsoft.VisualStudio.Workload.VCTools',
    '--includeRecommended', '--quiet', '--norestart', '--wait'
) -Wait -NoNewWindow -PassThru
$p.ExitCode | Out-File 'C:\vs-exit.txt'
", 1200)?;
        println!("  ✓ VS Build Tools installed");
    } else {
        println!("  ✓ VS Build Tools already installed");
    }

    // Step 5: WebView2 Runtime (required by Tauri)
    let wv2 = w.run_ps(
        "winget list --id Microsoft.EdgeWebView2Runtime --accept-source-agreements 2>$null | Select-String 'EdgeWebView2'",
    )?;
    if wv2.stdout.trim().is_empty() {
        println!("  Installing WebView2 Runtime...");
        w.run_elevated(
            "winget install --id Microsoft.EdgeWebView2Runtime --accept-source-agreements --accept-package-agreements --silent",
            120,
        )?;
        println!("  ✓ WebView2 Runtime installed");
    } else {
        println!("  ✓ WebView2 Runtime already installed");
    }

    // Step 6: mise (try winget first, fall back to PowerShell installer)
    let check_mise = w.run_ps(
        "Get-Command mise -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name",
    )?;
    if check_mise.stdout.trim().is_empty() {
        println!("  Installing mise...");
        let r = w.run_ps(
            "winget install --id jdx.mise --accept-source-agreements --accept-package-agreements --silent",
        )?;
        if r.exit_code != 0 {
            w.run_elevated("Invoke-Expression (Invoke-WebRequest -Uri 'https://mise.run' -UseBasicParsing).Content", 120)?;
        }
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

