//! Windows bootstrap — WinRM transport, PowerShell-driven.
//!
//! All multi-line PowerShell lives in `scripts/bootstrap/windows/*.ps1`,
//! pulled in via `include_str!()` so the binary stays single-file but the
//! scripts get editor syntax highlighting and can be hand-tested by copying
//! them into a Windows VM. One-line ad-hoc checks (Get-Service, Test-Path)
//! stay inline.

use anyhow::{Result, bail};

use crate::vm::profiles::{BootstrapMode, VmProfile};
use crate::vm::winrm;

const SCRIPT_SSHD_CONFIG: &str = include_str!("../../../scripts/bootstrap/windows/sshd-config.ps1");
const SCRIPT_AUTHORIZED_KEYS: &str =
    include_str!("../../../scripts/bootstrap/windows/authorized-keys.ps1");
const SCRIPT_VS_BUILDTOOLS_DOWNLOAD: &str =
    include_str!("../../../scripts/bootstrap/windows/vs-buildtools-download.ps1");
const SCRIPT_VS_BUILDTOOLS_INSTALL: &str =
    include_str!("../../../scripts/bootstrap/windows/vs-buildtools-install.ps1");
const SCRIPT_WEBVIEW2_INSTALL: &str =
    include_str!("../../../scripts/bootstrap/windows/webview2-install.ps1");
const SCRIPT_CARGO_BINSTALL_INSTALL: &str =
    include_str!("../../../scripts/bootstrap/windows/cargo-binstall-install.ps1");
const SCRIPT_MISE_CONFIG: &str = include_str!("../../../scripts/bootstrap/windows/mise-config.ps1");
const SCRIPT_RUSTUP_DEFAULT_HOST: &str =
    include_str!("../../../scripts/bootstrap/windows/rustup-default-host.ps1");
const SCRIPT_DEFENDER_EXCLUSIONS: &str =
    include_str!("../../../scripts/bootstrap/windows/defender-exclusions.ps1");

pub(super) fn run(profile: &VmProfile) -> Result<()> {
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
        let cap =
            w.run_ps("(Get-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0).State")?;
        if !cap.stdout.contains("Installed") {
            println!("  Installing OpenSSH Server (~3 min, downloads from Windows Update)...");
            w.run_elevated(
                "Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0",
                360,
            )?;
        }
        w.run_elevated(SCRIPT_SSHD_CONFIG, 60)?;
        println!("  ✓ OpenSSH Server installed and configured");
    } else {
        println!("  ✓ sshd already running");
    }

    // Step 2: Authorise the host's public key in BOTH ~/.ssh/authorized_keys
    // (for non-admin users) AND C:\ProgramData\ssh\administrators_authorized_keys
    // (the location Windows OpenSSH redirects admin users to via the default
    // `Match Group administrators` block in sshd_config). Step 1's regex
    // tries to comment that block out but doesn't always match cleanly — so
    // we install in both paths and stay compatible.
    let pub_key = super::find_public_key()?;
    w.run_ps(&SCRIPT_AUTHORIZED_KEYS.replace("__PUB_KEY__", &pub_key))?;
    println!("  ✓ SSH authorized key installed (both user + admin paths)");

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

    // Step 4: VS Build Tools with C++ workload + ARM64 cross-tools.
    // Idempotent check: look for Hostarm64\x64\link.exe — what we actually
    // depend on. Installing the workload places this on ARM64 hosts even
    // though the broader ARM64-native toolchain is BLOCKED_BY_MS.
    let vc_check = w.run_ps(
        r#"if (Get-ChildItem 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\*\bin\Hostarm64\x64\link.exe' -ErrorAction SilentlyContinue) { 'present' } else { 'missing' }"#
    )?;
    if vc_check.stdout.trim() != "present" {
        println!("  Downloading VS Build Tools bootstrapper...");
        w.run_ps(SCRIPT_VS_BUILDTOOLS_DOWNLOAD)?;
        println!(
            "  Installing VS Build Tools + C++ workload + ARM64 compiler (10-15 min on ARM64)..."
        );
        w.run_elevated(SCRIPT_VS_BUILDTOOLS_INSTALL, 1800)?;
        println!("  ✓ VS Build Tools installed (with ARM64 toolchain)");
    } else {
        println!("  ✓ VS Build Tools already installed (ARM64 toolchain present)");
    }

    // Step 5: WebView2 Runtime (required by Tauri at runtime).
    let wv2_path = r"C:\Program Files (x86)\Microsoft\EdgeWebView";
    let wv2_check = w.run_ps(&format!(
        "if (Test-Path '{wv2_path}') {{ 'installed' }} else {{ 'missing' }}"
    ))?;
    if wv2_check.stdout.trim() != "installed" {
        println!("  Installing WebView2 Runtime (Evergreen Bootstrapper, ~150 KB)...");
        w.run_ps(SCRIPT_WEBVIEW2_INSTALL)?;
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

    // Step 7a: install cargo-binstall directly so mise's cargo backend can
    // fetch prebuilt tauri-cli + similar from GitHub Releases instead of
    // compiling from source (bypassing `cargo install cargo-binstall`,
    // which would itself compile from source ~5 min).
    w.run_ps(SCRIPT_CARGO_BINSTALL_INSTALL)?;
    println!("  ✓ cargo-binstall installed (mise binstall fast-path enabled)");

    // Step 7b: persist mise's cargo_binstall = true setting (env var alone
    // doesn't survive across sessions).
    w.run_ps(SCRIPT_MISE_CONFIG)?;
    println!("  ✓ mise config: cargo_binstall = true");

    // Step 7: switch rustup default-host to x86_64 on ARM64 hosts (no
    // Hostarm64\arm64\link.exe ships; x86_64 toolchain links cleanly via
    // Hostarm64\x64\link.exe and runs under Windows ARM64's emulation).
    let arch = w.run_ps("$env:PROCESSOR_ARCHITECTURE")?;
    if arch.stdout.trim() == "ARM64" {
        w.run_ps(SCRIPT_RUSTUP_DEFAULT_HOST)?;
        println!("  ✓ rustup default-host set to x86_64-pc-windows-msvc (ARM64 host workaround)");
    }

    // Step N: Windows Defender exclusions for cargo/mise/build paths
    // (eliminates intermittent file-lock errors during cargo extract/compile).
    if w.run_elevated(SCRIPT_DEFENDER_EXCLUSIONS, 60).is_ok() {
        println!("  ✓ Windows Defender exclusions added (cargo/rustup/mise/target)");
    } else {
        // Non-fatal — the build will still work, just slower and may hit
        // intermittent file-lock errors. User can re-run `vm doctor` later
        // or add exclusions manually.
        println!("  ⚠ Defender exclusions skipped (Add-MpPreference failed — non-fatal)");
    }

    println!("✓ Windows bootstrap complete");
    Ok(())
}
