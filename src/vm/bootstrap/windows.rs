//! Windows bootstrap — WinRM transport, PowerShell-driven.

use anyhow::{Result, bail};

use crate::vm::profiles::{BootstrapMode, VmProfile};
use crate::vm::winrm;

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

    // Step 2: Authorise the host's public key. Windows OpenSSH has a default
    // `Match Group administrators` block in sshd_config that redirects admin
    // users (vagrant is admin on Vagrant boxes) to read keys from
    // C:\ProgramData\ssh\administrators_authorized_keys — NOT ~/.ssh/.
    // Step 1's regex-rewrite tries to comment that block out but the regex
    // doesn't always match cleanly across sshd_config variants, so we install
    // the key in BOTH paths and stay compatible regardless.
    let pub_key = super::find_public_key()?;
    let key_ps = format!(
        r#"
$key = '{pub_key}'

# User-level (works when Match Group administrators is commented out)
$dir = "$env:USERPROFILE\.ssh"
if (-not (Test-Path $dir)) {{ New-Item -ItemType Directory -Path $dir | Out-Null }}
$f = "$dir\authorized_keys"
if (-not (Test-Path $f) -or ((Get-Content $f -ErrorAction SilentlyContinue) -notcontains $key)) {{
    Add-Content $f $key -Encoding ASCII
}}
icacls $f /inheritance:r /grant ($env:USERNAME + ':F') /grant 'SYSTEM:F' | Out-Null

# Admin-level (always-honoured location for Match Group administrators users)
$adm = 'C:\ProgramData\ssh\administrators_authorized_keys'
if (-not (Test-Path $adm) -or ((Get-Content $adm -ErrorAction SilentlyContinue) -notcontains $key)) {{
    Add-Content $adm $key -Encoding ASCII
}}
icacls $adm /inheritance:r /grant 'Administrators:F' /grant 'SYSTEM:F' | Out-Null

Restart-Service sshd -ErrorAction SilentlyContinue
"#
    );
    w.run_ps(&key_ps)?;
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
    //
    // History: we used to require `Microsoft.VisualStudio.Component.VC.Tools.ARM64`
    // and feed that to `--add`. Both `vs_buildtools.exe` and `vs_installer.exe
    // modify` accept the flag and exit 0, but on ARM64 hosts they DO NOT
    // actually install Hostarm64\arm64 native tools — see GAPS.md #1
    // ("BLOCKED_BY_MS"). That made the check forever fail, so every vm up
    // re-ran the 100s installer for nothing.
    //
    // Idempotent-correct check: look for the actual binary we depend on,
    // `Hostarm64\x64\link.exe` — the ARM64-host x64-target cross-linker.
    // It IS installed by the VCTools workload + --includeRecommended on
    // ARM64 hosts. Once present, skip the installer entirely.
    let vc_check = w.run_ps(
        r#"if (Get-ChildItem 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\*\bin\Hostarm64\x64\link.exe' -ErrorAction SilentlyContinue) { 'present' } else { 'missing' }"#
    )?;
    if vc_check.stdout.trim() != "present" {
        println!("  Downloading VS Build Tools bootstrapper...");
        w.run_ps(r"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Invoke-WebRequest -Uri 'https://aka.ms/vs/17/release/vs_buildtools.exe' -OutFile 'C:\vs_buildtools.exe' -UseBasicParsing
")?;
        println!(
            "  Installing VS Build Tools + C++ workload + ARM64 compiler (10-15 min on ARM64)..."
        );
        w.run_elevated(
            r"
$p = Start-Process -FilePath 'C:\vs_buildtools.exe' -ArgumentList @(
    '--add', 'Microsoft.VisualStudio.Workload.VCTools',
    '--add', 'Microsoft.VisualStudio.Component.VC.Tools.ARM64',
    '--add', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
    '--add', 'Microsoft.VisualStudio.Component.Windows11SDK.22621',
    '--includeRecommended', '--quiet', '--norestart', '--wait'
) -Wait -NoNewWindow -PassThru
$p.ExitCode | Out-File 'C:\vs-exit.txt'
",
            1800,
        )?;
        println!("  ✓ VS Build Tools installed (with ARM64 toolchain)");
    } else {
        println!("  ✓ VS Build Tools already installed (ARM64 toolchain present)");
    }

    // Step 5: WebView2 Runtime (required by Tauri at runtime).
    //
    // We tried `winget install --id Microsoft.EdgeWebView2Runtime` first —
    // it consistently failed on fresh Vagrant Windows boxes ("No installed
    // package found matching input criteria") because winget's Store source
    // isn't always primed. The Evergreen Bootstrapper from
    // https://go.microsoft.com/fwlink/p/?LinkId=2124703 is the supported
    // headless install path Microsoft documents — small (~150 KB) and works
    // on minimal Windows.
    let wv2_path = r"C:\Program Files (x86)\Microsoft\EdgeWebView";
    let wv2_check = w.run_ps(&format!(
        "if (Test-Path '{wv2_path}') {{ 'installed' }} else {{ 'missing' }}"
    ))?;
    if wv2_check.stdout.trim() != "installed" {
        println!("  Installing WebView2 Runtime (Evergreen Bootstrapper, ~150 KB)...");
        w.run_ps(r#"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Invoke-WebRequest -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile 'C:\webview2_setup.exe' -UseBasicParsing
Start-Process 'C:\webview2_setup.exe' -ArgumentList '/silent','/install' -Wait -NoNewWindow
"#)?;
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

    // Step 7a: install cargo-binstall directly so mise's cargo backend
    // can use it (with MISE_CARGO_BINSTALL=true) to fetch prebuilt
    // tauri-cli + similar from GitHub Releases instead of compiling
    // from source. Direct .exe download — bypassing `cargo install
    // cargo-binstall` (which would itself compile from source ~5 min,
    // defeating the purpose).
    //
    // Asset: cargo-binstall-x86_64-pc-windows-msvc.zip (we run x86_64
    // binaries on this ARM64 VM via emulation, since rustup is
    // configured for x86_64-host already).
    w.run_ps(r#"
$dest = "$env:USERPROFILE\.cargo\bin"
if (-not (Test-Path "$dest\cargo-binstall.exe")) {
    if (-not (Test-Path $dest)) { New-Item -ItemType Directory -Path $dest | Out-Null }
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $zip = "$env:TEMP\cargo-binstall.zip"
    Invoke-WebRequest -Uri 'https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-x86_64-pc-windows-msvc.zip' -OutFile $zip -UseBasicParsing
    Expand-Archive -Force $zip $dest
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
}
# Ensure ~/.cargo/bin is on PATH for future shell sessions.
$path = [Environment]::GetEnvironmentVariable('PATH','User')
if ($path -notmatch [regex]::Escape($dest)) {
    [Environment]::SetEnvironmentVariable('PATH', $path + ';' + $dest, 'User')
}
"#)?;
    println!("  ✓ cargo-binstall installed (mise binstall fast-path enabled)");

    // Step 7b: persist mise's `cargo_binstall = true` setting in the user
    // mise config. Belt and braces alongside MISE_CARGO_BINSTALL env var
    // — env vars are scoped to a process, settings file is permanent.
    w.run_ps(
        r#"
$cfg = "$env:USERPROFILE\AppData\Roaming\mise\config.toml"
$dir = Split-Path $cfg
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
if (-not (Test-Path $cfg)) {
    Set-Content $cfg "[settings]`ncargo_binstall = true`n" -Encoding UTF8
} elseif ((Get-Content $cfg -Raw) -notmatch 'cargo_binstall') {
    Add-Content $cfg "`n[settings]`ncargo_binstall = true`n" -Encoding UTF8
}
"#,
    )?;
    println!("  ✓ mise config: cargo_binstall = true");

    // Step 7: switch rustup default-host to x86_64.
    //
    // VS Build Tools on ARM64 Windows hosts ships only Hostarm64\x64 and
    // Hostarm64\x86 cross-tools — there's no Hostarm64\arm64 native
    // toolchain at the time of writing. So a project's `mise.toml` declaring
    // `rust = "stable"` would otherwise install the host-default
    // `aarch64-pc-windows-msvc` toolchain, which fails to link anything
    // (no ARM64 link.exe). By forcing rustup's default-host to x86_64 here
    // BEFORE any project's mise install runs, the toolchain mise installs
    // is x86_64 — which links cleanly with Hostarm64\x64\link.exe and runs
    // under Windows ARM64's native x64 emulation.
    //
    // This is the one place we touch a runtime tool's config, and only
    // because the alternative is "Windows builds don't work at all on this
    // VM until each user discovers and works around it themselves."
    let arch = w.run_ps("$env:PROCESSOR_ARCHITECTURE")?;
    if arch.stdout.trim() == "ARM64" {
        // Use rustup that mise wires up. mise installs rust under
        // %USERPROFILE%\.local\share\mise\installs\rust\<ver>\rustup.exe or
        // (on this image) D:\mise\installs\rust\stable\rustup.exe — cover
        // both. We don't run this if rustup isn't anywhere yet; mise will
        // place it later and we'll set the host then. But if rust is
        // already managed by mise, switch the default host now.
        w.run_ps(
            r#"
$candidates = @(
  'D:\mise\installs\rust\stable\rustup.exe',
  "$env:USERPROFILE\.local\share\mise\installs\rust\stable\rustup.exe"
)
$rustup = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($rustup) {
  & $rustup set default-host x86_64-pc-windows-msvc
  & $rustup default --force-non-host stable-x86_64-pc-windows-msvc
}
"#,
        )?;
        println!("  ✓ rustup default-host set to x86_64-pc-windows-msvc (ARM64 host workaround)");
    }

    // Step N: Windows Defender exclusions for cargo/mise/build paths.
    //
    // Why: Defender's real-time scan briefly locks freshly-written files
    // during cargo extract/compile. The lock manifests as
    // "The process cannot access the file because it is being used by
    // another process" partway through `mise install` or `cargo build`.
    // Adding exclusions for the build dirs eliminates this — standard
    // practice across the Rust/JetBrains/Microsoft dev-VM ecosystem.
    //
    // Idempotent: Add-MpPreference no-ops if the path is already excluded.
    let defender_ps = r#"
$paths = @(
  'D:\target',
  "$env:USERPROFILE\.cargo",
  "$env:USERPROFILE\.rustup",
  "$env:USERPROFILE\.local\share\mise",
  "$env:USERPROFILE\AppData\Local\mise",
  "$env:USERPROFILE\.utm-dev-build"
)
foreach ($p in $paths) {
  try { Add-MpPreference -ExclusionPath $p -ErrorAction Stop } catch { }
}
"#;
    if w.run_elevated(defender_ps, 60).is_ok() {
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
