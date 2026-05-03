/// First-run bootstrap: installs build tools, Rust, and mise in a fresh VM.
/// Idempotent — safe to run multiple times (checks before installing).
use anyhow::{bail, Context, Result};

use super::profiles::{BootstrapMode, GuestOs, VmProfile};
use super::{ssh, winrm};

pub fn run(profile: &VmProfile) -> Result<()> {
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

fn linux(session: &ssh::Session, profile: &VmProfile) -> Result<()> {
    println!("→ Bootstrapping Linux VM (mode: {:?})...", profile.bootstrap);

    // Install host's public key so the user can `code --remote ssh-remote+...`
    // (and re-runs of `vm exec`) without password prompts. Idempotent.
    install_host_pubkey(session)?;

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

    // Step 2: Tauri Linux dependencies + observability tools.
    //
    // We check Xvfb specifically (not libwebkit2gtk) because Xvfb is the
    // newest addition — older bootstrapped VMs WILL have libwebkit2gtk
    // already and would skip the whole apt-install if we keyed on that,
    // missing xvfb / scrot. apt-get install is idempotent — passing tools
    // that are already installed is a no-op, fast (~1s).
    let xvfb_present = ssh::exec(session,
        "command -v Xvfb >/dev/null 2>&1 && echo present || echo missing"
    ).unwrap_or_default();
    if !xvfb_present.contains("present") {
        // libwebkit2gtk + GTK family — Tauri build deps.
        // xvfb: virtual framebuffer X server for headless GUI launches
        //       (`vm run` uses Xvfb on :99 so apps boot without a display).
        // scrot: tiny screenshot tool — `vm screenshot` captures the
        //        xvfb display and scp's the png back.
        // xdg-utils: xdg-open, required by Tauri's AppImage bundler.
        // openbox: tiny window manager (~1 MB). Without a WM, GTK windows
        // open but don't get mapped/composited on bare Xvfb, so vm screenshot
        // returns a black png. With openbox running on :99, windows appear.
        run_step(session, "install Tauri Linux deps + xvfb + scrot + openbox",
            "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
             libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
             librsvg2-dev libssl-dev libxdo-dev patchelf wget file \
             libsoup-3.0-dev libjavascriptcoregtk-4.1-dev xvfb xdg-utils \
             scrot openbox")?;
    } else {
        println!("  ✓ Tauri deps + xvfb + scrot already installed");
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

    // (Rust is installed by the project's mise.toml at vm build time, not
    // here — see AGENTS.md "Source-of-truth invariant for VM bootstrap".)

    // Step 3b: cargo-binstall + mise's cargo_binstall setting.
    // mirrors what the Windows bootstrap (step 7a/7b) does — installs the
    // cargo-binstall binary directly (no compile) and persists the mise
    // setting so cargo: tools fetch prebuilt binaries from GitHub releases
    // instead of compiling from source.
    let binstall_present = ssh::exec(session,
        "[ -x \"$HOME/.cargo/bin/cargo-binstall\" ] && echo present || echo missing"
    ).unwrap_or_default();
    if !binstall_present.contains("present") {
        let arch = ssh::exec(session, "uname -m").unwrap_or_default();
        let target = if arch.trim() == "aarch64" {
            "aarch64-unknown-linux-musl"
        } else {
            "x86_64-unknown-linux-musl"
        };
        let url = format!(
            "https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-{target}.tgz"
        );
        run_step(session, "install cargo-binstall (binstall fast-path)",
            &format!("mkdir -p ~/.cargo/bin && curl -sSfL {url} | tar -xz -C ~/.cargo/bin && chmod +x ~/.cargo/bin/cargo-binstall"))?;
    } else {
        println!("  ✓ cargo-binstall already installed");
    }
    // mise config: cargo_binstall = true (idempotent).
    run_step(session, "configure mise cargo_binstall = true",
        "mkdir -p ~/.config/mise && \
         touch ~/.config/mise/config.toml && \
         (grep -q 'cargo_binstall' ~/.config/mise/config.toml || \
          printf '\\n[settings]\\ncargo_binstall = true\\n' >> ~/.config/mise/config.toml)")?;

    // Step 4: linux-dev extras (Debian 12 with GNOME).
    // Marker is fonts-noto-color-emoji because xdg-utils is already
    // installed for ALL Linux profiles by step 2.
    if profile.name == "linux-dev" {
        let emoji = ssh::exec(session,
            "dpkg -s fonts-noto-color-emoji 2>/dev/null | grep -c 'ok installed'"
        ).unwrap_or_default();
        if emoji.trim() != "1" {
            run_step(session, "install desktop extras (fonts-noto-color-emoji)",
                "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
                 fonts-noto-color-emoji")?;
        } else {
            println!("  ✓ desktop extras already installed");
        }
    }

    println!("✓ Linux bootstrap complete");
    Ok(())
}

fn run_step(session: &ssh::Session, label: &str, cmd: &str) -> Result<()> {
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

fn install_host_pubkey(session: &ssh::Session) -> Result<()> {
    let pub_key = match find_public_key() {
        Ok(k) => k,
        Err(_) => {
            println!("  ⚠ no SSH public key in ~/.ssh — VS Code Remote SSH will prompt for password");
            return Ok(());
        }
    };
    // Quote-safe single-line shell pipeline. grep -qxF avoids partial-line matches.
    let cmd = format!(
        "mkdir -p ~/.ssh && chmod 700 ~/.ssh && touch ~/.ssh/authorized_keys && \
         chmod 600 ~/.ssh/authorized_keys && \
         grep -qxF {key} ~/.ssh/authorized_keys || echo {key} >> ~/.ssh/authorized_keys",
        key = shell_quote(&pub_key)
    );
    let (out, code) = ssh::exec_with_exit(session, &cmd)?;
    if code != 0 {
        println!("  ⚠ failed to install host pubkey (exit {code}): {out}");
    } else {
        println!("  ✓ host SSH key authorised (passwordless `code --remote` ready)");
    }
    Ok(())
}

fn shell_quote(s: &str) -> String {
    // Wrap in single quotes; escape any embedded single quotes by closing,
    // adding an escaped quote, and reopening.
    format!("'{}'", s.replace('\'', r"'\''"))
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

    // Step 2: Authorise the host's public key. Windows OpenSSH has a default
    // `Match Group administrators` block in sshd_config that redirects admin
    // users (vagrant is admin on Vagrant boxes) to read keys from
    // C:\ProgramData\ssh\administrators_authorized_keys — NOT ~/.ssh/.
    // Step 1's regex-rewrite tries to comment that block out but the regex
    // doesn't always match cleanly across sshd_config variants, so we install
    // the key in BOTH paths and stay compatible regardless.
    let pub_key = find_public_key()?;
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
        println!("  Installing VS Build Tools + C++ workload + ARM64 compiler (10-15 min on ARM64)...");
        w.run_elevated(r"
$p = Start-Process -FilePath 'C:\vs_buildtools.exe' -ArgumentList @(
    '--add', 'Microsoft.VisualStudio.Workload.VCTools',
    '--add', 'Microsoft.VisualStudio.Component.VC.Tools.ARM64',
    '--add', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
    '--add', 'Microsoft.VisualStudio.Component.Windows11SDK.22621',
    '--includeRecommended', '--quiet', '--norestart', '--wait'
) -Wait -NoNewWindow -PassThru
$p.ExitCode | Out-File 'C:\vs-exit.txt'
", 1800)?;
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
    w.run_ps(r#"
$cfg = "$env:USERPROFILE\AppData\Roaming\mise\config.toml"
$dir = Split-Path $cfg
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
if (-not (Test-Path $cfg)) {
    Set-Content $cfg "[settings]`ncargo_binstall = true`n" -Encoding UTF8
} elseif ((Get-Content $cfg -Raw) -notmatch 'cargo_binstall') {
    Add-Content $cfg "`n[settings]`ncargo_binstall = true`n" -Encoding UTF8
}
"#)?;
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
        w.run_ps(r#"
$candidates = @(
  'D:\mise\installs\rust\stable\rustup.exe',
  "$env:USERPROFILE\.local\share\mise\installs\rust\stable\rustup.exe"
)
$rustup = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($rustup) {
  & $rustup set default-host x86_64-pc-windows-msvc
  & $rustup default --force-non-host stable-x86_64-pc-windows-msvc
}
"#)?;
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

