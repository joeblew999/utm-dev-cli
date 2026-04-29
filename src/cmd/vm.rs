use clap::Subcommand;

use crate::cli::BuildTarget;
use crate::vm::{bootstrap, build, import, profiles, ssh, state, utm};

#[derive(Subcommand)]
pub enum VmCommands {
    /// Start a VM (imports box + bootstraps on first run, then just starts)
    Up {
        #[arg(long, help = "VM profile name (windows-build | linux-build | linux-dev | …)")]
        name: String,
    },
    /// Stop a VM
    Down {
        #[arg(long, help = "VM profile name")]
        name: String,
    },
    /// Restart a VM (vm down + vm up; preserves bootstrap state)
    Restart {
        #[arg(long, help = "VM profile name")]
        name: String,
    },
    /// Build app in a VM (auto-starts if needed, syncs code, pulls artifacts)
    Build {
        #[arg(long, help = "VM profile name")]
        name: String,
        #[arg(long, value_enum, default_value_t = BuildTarget::Both,
              help = "Architecture: arm64 | x86-64 | both")]
        target: BuildTarget,
        #[arg(long, help = "Optimised release build")]
        release: bool,
    },
    /// Launch a built binary inside the VM and capture its startup output.
    /// Run after `vm build`. Tail with `vm logs --kind run --follow`.
    Run {
        #[arg(long, help = "VM profile name")]
        name: String,
        /// Path to the binary on the VM (absolute or relative to the project dir).
        /// If omitted, auto-detects the most recent bundle for the host's arch.
        #[arg(long)]
        bin: Option<String>,
    },
    /// Capture the VM's display and pull a PNG back to the host.
    /// Linux only for now (uses scrot against the xvfb display from `vm run`).
    Screenshot {
        #[arg(long, help = "VM profile name")]
        name: String,
        #[arg(long, default_value = "screenshot.png", help = "Local path for the .png")]
        out: String,
    },
    /// Run a command in a VM via SSH
    Exec {
        #[arg(long, help = "VM profile name")]
        name: String,
        /// Command to run (quoted as a single string)
        cmd: Vec<String>,
    },
    /// Open an interactive SSH shell in a VM
    Shell {
        #[arg(long, help = "VM profile name")]
        name: String,
    },
    /// Tail logs from inside the VM (build, or runtime via vm run)
    Logs {
        #[arg(long, help = "VM profile name")]
        name: String,
        #[arg(long, default_value = "build", help = "Which log: build | run")]
        kind: String,
        #[arg(long, help = "Follow (tail -f) instead of dumping the full log")]
        follow: bool,
        /// Show only the last N lines (handy when builds fail — error is usually in the tail).
        #[arg(long)]
        tail: Option<u32>,
        /// Filter to error stanzas with surrounding context. The fast path for "why did my build fail".
        #[arg(long)]
        errors: bool,
    },
    /// Run health checks inside the VM (mise, rust toolchain, VS components, …).
    /// Use to debug "why didn't my build work" without wading through full logs.
    Doctor {
        #[arg(long, help = "VM profile name")]
        name: String,
    },
    /// Copy file or directory from host to VM (scp -r)
    Push {
        #[arg(long, help = "VM profile name")]
        name: String,
        #[arg(long, help = "Local path on host")]
        from: String,
        #[arg(long, help = "Destination path on VM")]
        to:   String,
    },
    /// Copy file or directory from VM to host (scp -r)
    Pull {
        #[arg(long, help = "VM profile name")]
        name: String,
        #[arg(long, help = "Source path on VM")]
        from: String,
        #[arg(long, help = "Local destination path on host")]
        to:   String,
    },
    /// Wire an existing UTM VM to a profile (skips download/import)
    Adopt {
        #[arg(long, help = "Profile name to assign (windows-build | linux-build | …)")]
        name: String,
        #[arg(long, help = "Exact display name of the VM in UTM (from utmctl list)")]
        utm_name: String,
    },
    /// Delete a VM from UTM
    Delete {
        #[arg(long, help = "VM profile name, or 'all' to remove all utm-dev VMs")]
        name: String,
    },
    /// Export a VM as a Vagrant box for distribution
    Package {
        #[arg(long, help = "VM profile name")]
        name: String,
    },
    /// Grow the VM's primary qcow2 disk + extend the guest partition.
    /// VM must be stopped. After resize, restart with `vm up`.
    ResizeDisk {
        #[arg(long, help = "VM profile name")]
        name: String,
        #[arg(long, default_value = "60", help = "Additional gigabytes to add")]
        plus_gb: u32,
    },
    /// List available VM profiles
    Ls,
}

pub fn run(cmd: VmCommands) -> anyhow::Result<()> {
    match cmd {
        VmCommands::Up { name }             => vm_up(&name),
        VmCommands::Down { name }           => vm_down(&name),
        VmCommands::Restart { name }        => { vm_down(&name)?; vm_up(&name) }
        VmCommands::Exec { name, cmd }      => vm_exec(&name, &cmd.join(" ")),
        VmCommands::Shell { name }          => vm_shell(&name),
        VmCommands::Logs { name, kind, follow, tail, errors } => vm_logs(&name, &kind, follow, tail, errors),
        VmCommands::Doctor { name }         => vm_doctor(&name),
        VmCommands::Push { name, from, to } => vm_push(&name, &from, &to),
        VmCommands::Pull { name, from, to } => vm_pull(&name, &from, &to),
        VmCommands::Adopt { name, utm_name } => vm_adopt(&name, &utm_name),
        VmCommands::Ls                      => vm_ls(),
        VmCommands::Build { name, target, release } => vm_build(&name, target, release),
        VmCommands::Run { name, bin }       => vm_run(&name, bin.as_deref()),
        VmCommands::Screenshot { name, out } => vm_screenshot(&name, &out),
        VmCommands::Delete { name }         => vm_delete(&name),
        VmCommands::Package { name }        => vm_package(&name),
        VmCommands::ResizeDisk { name, plus_gb } => vm_resize_disk(&name, plus_gb),
    }
}

fn ensure_qemu_img() -> anyhow::Result<String> {
    if let Ok(p) = which::which("qemu-img") {
        return Ok(p.to_string_lossy().into_owned());
    }
    println!("→ qemu-img not found — installing qemu via brew (~50 MB, one-time)...");
    let r = std::process::Command::new("brew")
        .args(["install", "qemu"])
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .status()
        .map_err(|e| anyhow::anyhow!("brew not found or failed: {e}"))?;
    if !r.success() {
        anyhow::bail!("brew install qemu failed");
    }
    let p = which::which("qemu-img")
        .map_err(|_| anyhow::anyhow!("qemu-img still not on PATH after brew install"))?;
    println!("✓ qemu-img: {}", p.display());
    Ok(p.to_string_lossy().into_owned())
}

// ── vm resize-disk ────────────────────────────────────────────────────────────

fn vm_resize_disk(name: &str, plus_gb: u32) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    let st = state::load(name)
        .map_err(|_| anyhow::anyhow!("'{name}' not imported — run: utm-dev vm up --name {name}"))?;

    // VM must be stopped — resizing a running qcow2 corrupts it.
    let running = utm::list_vms()
        .unwrap_or_default()
        .into_iter()
        .any(|e| e.name == st.display_name && e.status == "started");
    if running {
        println!("→ Stopping {} (must be off to resize disk)...", st.display_name);
        utm::stop_vm(&st.display_name)?;
        std::thread::sleep(std::time::Duration::from_secs(8));
    }

    // Locate the qcow2: ~/Library/.../Documents/<display>.utm/Data/<uuid>.qcow2
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let bundle = home
        .join("Library/Containers/com.utmapp.UTM/Data/Documents")
        .join(format!("{}.utm", st.display_name))
        .join("Data");
    let qcow2 = std::fs::read_dir(&bundle)?
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().map(|x| x == "qcow2").unwrap_or(false))
        .ok_or_else(|| anyhow::anyhow!("no .qcow2 found in {}", bundle.display()))?
        .path();
    println!("→ qcow2: {}", qcow2.display());

    // UTM bundles qemu-img only as a dylib, not a runnable CLI. Use the
    // standalone qemu Homebrew package instead. Auto-install if missing.
    let qemu_img = ensure_qemu_img()?;

    // Get current size (info JSON)
    let info = std::process::Command::new(&qemu_img)
        .args(["info", "--output=json"])
        .arg(&qcow2)
        .output()
        .map_err(|e| anyhow::anyhow!("qemu-img info: {e}"))?;
    if !info.status.success() {
        anyhow::bail!(
            "qemu-img info failed: {}",
            String::from_utf8_lossy(&info.stderr)
        );
    }
    let info_text = String::from_utf8_lossy(&info.stdout);
    let virtual_gb = info_text
        .split("\"virtual-size\":")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|b| b as f64 / 1_073_741_824.0);
    if let Some(gb) = virtual_gb {
        println!("  current virtual size: {:.1} GB", gb);
    }

    println!("→ Growing qcow2 by +{plus_gb}G...");
    let status = std::process::Command::new(qemu_img)
        .args(["resize"])
        .arg(&qcow2)
        .arg(format!("+{plus_gb}G"))
        .status()
        .map_err(|e| anyhow::anyhow!("qemu-img resize: {e}"))?;
    if !status.success() {
        anyhow::bail!("qemu-img resize failed");
    }
    println!("✓ qcow2 grown");

    println!(
        "→ Now: utm-dev vm up --name {name}\n\
         Then to extend the partition inside the guest:"
    );
    match profile.os {
        profiles::GuestOs::Windows => {
            println!(
                "    utm-dev vm exec --name {name} 'powershell -NoProfile -Command \"Resize-Partition -DriveLetter C -Size (Get-PartitionSupportedSize -DriveLetter C).SizeMax\"'"
            );
        }
        profiles::GuestOs::Linux => {
            println!(
                "    utm-dev vm exec --name {name} 'sudo growpart /dev/vda 1 && sudo resize2fs /dev/vda1'"
            );
        }
    }
    Ok(())
}

// ── vm adopt ─────────────────────────────────────────────────────────────────

fn vm_adopt(name: &str, utm_name: &str) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;

    // Find the VM in UTM by display name
    let entry = utm::list_vms()?
        .into_iter()
        .find(|e| e.name == utm_name)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "VM '{}' not found in UTM. Run `utmctl list` to see available VMs.",
                utm_name
            )
        })?;

    println!(
        "✓ Found '{}' in UTM (UUID: {}, status: {})",
        utm_name, entry.uuid, entry.status
    );

    // Validate that SSH port forward is in place
    println!("→ Checking SSH port {} is reachable...", profile.ssh_port);
    match ssh::check(profile) {
        Ok(()) => println!("✓ SSH port {} is open", profile.ssh_port),
        Err(e) => println!(
            "  ⚠ SSH check failed (VM may be stopped): {e}\n  Start the VM and re-run `vm adopt` to verify, or continue and try `vm up`."
        ),
    }

    state::save(
        name,
        &state::VmState {
            uuid:         entry.uuid.clone(),
            display_name: utm_name.to_string(),
        },
    )?;

    println!("✓ Adopted: profile '{}' → UTM VM '{}' ({})", name, utm_name, entry.uuid);
    println!("  Run: utm-dev vm up --name {name}");
    Ok(())
}

// ── vm up ─────────────────────────────────────────────────────────────────────

fn vm_up(name: &str) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;

    utm::ensure_utm()?;

    // Resolve actual UTM display name: state file wins over profile.box_name
    let (uuid, utm_display) = if state::exists(name) {
        let st = state::load(name)?;
        (st.uuid, st.display_name)
    } else {
        // First run: import (download + import box). UTM may rename the bundle
        // based on its internal _config.plist (e.g. windows-11 → packer-vm-…),
        // so persist the *actual* display name UTM assigned, not profile.box_name.
        println!("── First run for '{}' ─────────────────────────", name);
        let (uuid, display) = import::ensure_imported(profile)?;
        utm::configure_network(&uuid, profile)?;
        utm::configure_resources(&uuid, profile.memory_mib, profile.cpu_cores)?;
        state::save(
            name,
            &state::VmState {
                uuid:         uuid.clone(),
                display_name: display.clone(),
            },
        )?;
        (uuid, display)
    };

    // Start and wait for boot. wait_for_boot probes the right service per OS:
    // SSH for Linux, WinRM for Windows. For Windows on first boot, OpenSSH
    // isn't installed yet — the WinRM bootstrap is what installs it.
    utm::start_vm(&utm_display)?;
    utm::wait_for_boot(profile, 300)?;

    // Bootstrap (idempotent — checks before each step). Windows uses WinRM
    // internally; Linux opens its own SSH session inside bootstrap::run.
    bootstrap::run(profile)?;

    println!("✓ {} is up (UUID: {})", name, uuid);
    Ok(())
}

// ── vm down ──────────────────────────────────────────────────────────────────

fn vm_down(name: &str) -> anyhow::Result<()> {
    let _profile = profiles::get(name)?; // validate profile name

    // Use actual UTM display name from state if available, otherwise profile.box_name
    let utm_display = if state::exists(name) {
        state::load(name)?.display_name
    } else {
        profiles::get(name)?.box_name.to_string()
    };

    utm::stop_vm(&utm_display)?;
    println!("✓ {} is down", name);
    Ok(())
}

// ── vm exec ──────────────────────────────────────────────────────────────────

fn vm_exec(name: &str, cmd: &str) -> anyhow::Result<()> {
    if cmd.is_empty() {
        anyhow::bail!("vm exec requires a command");
    }
    let profile = profiles::get(name)?;
    ssh::check(profile)?;
    let session = ssh::connect(profile)?;
    let (out, code) = ssh::exec_with_exit(&session, cmd)?;
    if !out.is_empty() {
        println!("{out}");
    }
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

// ── vm shell / vm push / vm pull (delegate to ssh + scp) ────────────────────

fn vm_logs(
    name: &str,
    kind: &str,
    follow: bool,
    tail: Option<u32>,
    errors: bool,
) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    ssh::check(profile)?;

    if follow && errors {
        anyhow::bail!("--follow and --errors are mutually exclusive");
    }

    let log_path = match (kind, &profile.os) {
        ("build", profiles::GuestOs::Linux)   => "~/.utm-dev-build/build.log".to_string(),
        ("build", profiles::GuestOs::Windows) => r"%USERPROFILE%\.utm-dev-build\build.log".to_string(),
        ("run",   profiles::GuestOs::Linux)   => "~/.utm-dev-run/run.log".to_string(),
        ("run",   profiles::GuestOs::Windows) => r"%USERPROFILE%\.utm-dev-run\run.log".to_string(),
        _ => anyhow::bail!("unknown kind '{kind}' (expected: build | run)"),
    };

    // Pattern shared between OSes: cargo / rust / mise / Tauri / MSVC / linker errors.
    // Case-insensitive, line-anchored where useful.
    let cmd = if errors {
        match &profile.os {
            profiles::GuestOs::Linux => format!(
                // -E extended regex, -i ignore-case, -A/-B context, -n line numbers.
                // Patterns chosen to catch cargo/rustc/mise/MSVC/linker without being noisy.
                "grep -niE -A 5 -B 1 \
                 '(^error[:[ ]|^error\\[E[0-9]+\\]|^FAILED|^Failed |panic|fatal error|mise ERROR|unresolved external symbol|LNK[0-9]+|LNK4272|cannot find -l|linker .* not found)' \
                 {log_path} 2>/dev/null || echo '(no errors found in {log_path} — try `vm logs --tail 200`)'"
            ),
            profiles::GuestOs::Windows => format!(
                r#"powershell -NoProfile -Command "if (Test-Path '{log_path}') {{ \
                    $hits = Get-Content '{log_path}' | Select-String -Pattern '^error[:[ ]|^error\[E[0-9]+\]|^FAILED|panic|fatal error|mise ERROR|unresolved external symbol|LNK[0-9]+|cannot find -l|linker .* not found' -Context 1,5 -CaseSensitive:$false; \
                    if ($hits) {{ $hits | ForEach-Object {{ $_.Context.PreContext + $_.Line + $_.Context.PostContext + '---' }} }} else {{ '(no errors found in {log_path} — try `vm logs --tail 200`)' }} \
                  }} else {{ '(no log yet)' }}""#
            ),
        }
    } else {
        match (follow, tail, &profile.os) {
            (true,  _,        profiles::GuestOs::Linux)   => format!("tail -F {log_path} 2>/dev/null"),
            (false, Some(n),  profiles::GuestOs::Linux)   => format!("tail -n {n} {log_path} 2>/dev/null || echo '(no log yet)'"),
            (false, None,     profiles::GuestOs::Linux)   => format!("cat {log_path} 2>/dev/null || echo '(no log yet)'"),
            (true,  _,        profiles::GuestOs::Windows) => format!(
                r#"powershell -NoProfile -Command "Get-Content '{log_path}' -Wait -Tail 1000""#
            ),
            (false, Some(n),  profiles::GuestOs::Windows) => format!(
                r#"powershell -NoProfile -Command "if (Test-Path '{log_path}') {{ Get-Content '{log_path}' -Tail {n} }} else {{ '(no log yet)' }}""#
            ),
            (false, None,     profiles::GuestOs::Windows) => format!(
                r#"powershell -NoProfile -Command "if (Test-Path '{log_path}') {{ Get-Content '{log_path}' }} else {{ '(no log yet)' }}""#
            ),
        }
    };

    let code = ssh::exec_streaming(profile, &cmd)?;
    if code != 0 && !follow {
        std::process::exit(code);
    }
    Ok(())
}

// ── vm doctor ────────────────────────────────────────────────────────────────

fn vm_doctor(name: &str) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    let session = ssh::connect(profile)
        .map_err(|e| anyhow::anyhow!("cannot SSH to '{name}': {e:#}\n  → utm-dev vm up --name {name}"))?;

    println!("══ utm-dev vm doctor — {} ══\n", name);

    let checks: Vec<(&str, &str)> = match profile.os {
        profiles::GuestOs::Linux => vec![
            ("mise on PATH",
             "command -v mise >/dev/null && mise --version || echo MISSING"),
            ("apt build-essential",
             "dpkg-query -W -f='${Status}' build-essential 2>/dev/null | grep -c 'ok installed' | grep -qx 1 && echo ok || echo MISSING"),
            ("apt libwebkit2gtk-4.1-dev (Tauri)",
             "dpkg-query -W -f='${Status}' libwebkit2gtk-4.1-dev 2>/dev/null | grep -c 'ok installed' | grep -qx 1 && echo ok || echo MISSING"),
            ("apt libwebkit2gtk-4.1-dev:amd64 (multiarch x86_64)",
             "dpkg-query -W -f='${Status}' libwebkit2gtk-4.1-dev:amd64 2>/dev/null | grep -c 'ok installed' | grep -qx 1 && echo ok || echo 'MISSING (run vm build --target x86-64 to install)'"),
            ("apt gcc-x86-64-linux-gnu (cross linker)",
             "command -v x86_64-linux-gnu-gcc >/dev/null && echo ok || echo MISSING"),
            ("xvfb-run (vm run)",
             "command -v xvfb-run >/dev/null && echo ok || echo MISSING"),
        ],
        profiles::GuestOs::Windows => vec![
            ("mise on PATH",
             "where mise 2>nul && mise --version || echo MISSING"),
            ("VS Build Tools install path",
             r#"if exist "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC" (echo ok) else (echo MISSING)"#),
            ("VS Hostarm64\\x64 cross-tools (link.exe)",
             r#"for /d %V in ("C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\*") do @if exist "%V\bin\Hostarm64\x64\link.exe" echo ok"#),
            ("VS Hostarm64\\arm64 native tools (BLOCKED)",
             r#"for /d %V in ("C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\*") do @if exist "%V\bin\Hostarm64\arm64\link.exe" (echo ok) else (echo BLOCKED_BY_MS)"#),
            ("WebView2 Runtime",
             r#"if exist "C:\Program Files (x86)\Microsoft\EdgeWebView" (echo ok) else (echo MISSING)"#),
            ("rustup default-host = x86_64",
             r#"powershell -NoProfile -Command "$r = (mise where rust 2>$null); if ($r) { & ($r + '\\rustup.exe') show 2>$null | Select-String 'Default host:.*x86_64' | ForEach-Object { 'ok' } } else { 'MISSING' }""#),
        ],
    };

    let mut real_failures = 0;
    let mut expected_failures = 0;
    for (label, cmd) in checks {
        let out = ssh::exec(&session, cmd).unwrap_or_else(|e| format!("ERR {e}"));
        let trimmed = out.trim();
        let blocked = trimmed.contains("BLOCKED_BY_MS");
        let pass = !trimmed.is_empty()
            && !trimmed.contains("MISSING")
            && !blocked
            && !trimmed.starts_with("ERR")
            && !trimmed.contains("could not find")
            && !trimmed.contains("not recognized");
        if pass {
            println!("  ✓ {label}");
        } else if blocked {
            // Known limitation outside our control — surface but don't count as a real failure.
            println!("  ⚠ {label} (known-blocked, not actionable)");
            expected_failures += 1;
        } else {
            real_failures += 1;
            println!("  ✗ {label}");
            for line in trimmed.lines().take(3) {
                println!("      {line}");
            }
        }
    }

    println!();
    if real_failures == 0 {
        if expected_failures > 0 {
            println!("✓ all actionable checks passed ({expected_failures} known-blocked, see GAPS.md)");
        } else {
            println!("✓ all checks passed");
        }
    } else {
        println!("✗ {real_failures} check(s) failed");
        std::process::exit(1);
    }
    Ok(())
}

fn vm_shell(name: &str) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    ssh::check(profile)?;
    let target = format!("{}@localhost", profile.user);
    let status = std::process::Command::new("ssh")
        .args(["-p", &profile.ssh_port.to_string(), "-t",
               "-o", "StrictHostKeyChecking=no",
               "-o", "UserKnownHostsFile=/dev/null",
               "-o", "LogLevel=ERROR"])
        .arg(&target)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to spawn ssh: {e}"))?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn vm_push(name: &str, from: &str, to: &str) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    ssh::check(profile)?;
    println!("→ Pushing {} → {}:{}", from, profile.name, to);
    scp_run(profile, from, &format!("{}@localhost:{}", profile.user, to))
}

fn vm_pull(name: &str, from: &str, to: &str) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    ssh::check(profile)?;
    println!("→ Pulling {}:{} → {}", profile.name, from, to);
    scp_run(profile, &format!("{}@localhost:{}", profile.user, from), to)
}

fn scp_run(profile: &profiles::VmProfile, src: &str, dst: &str) -> anyhow::Result<()> {
    let status = std::process::Command::new("scp")
        .args(["-r", "-P", &profile.ssh_port.to_string(),
               "-o", "StrictHostKeyChecking=no",
               "-o", "UserKnownHostsFile=/dev/null",
               "-o", "LogLevel=ERROR",
               src, dst])
        .status()
        .map_err(|e| anyhow::anyhow!("failed to spawn scp: {e}"))?;
    if !status.success() {
        anyhow::bail!("scp exited {}", status);
    }
    println!("✓ done");
    Ok(())
}

// ── vm ls ─────────────────────────────────────────────────────────────────────

fn vm_ls() -> anyhow::Result<()> {
    // Try to get live UTM status
    let live = utm::list_vms().unwrap_or_default();

    println!("{:<18} {:<10} {:<8} {:<8} {:<12} {}", "NAME", "OS", "SSH", "RAM", "UTM STATUS", "UTM NAME");
    println!("{}", "-".repeat(72));
    for p in profiles::list() {
        let utm_name = if state::exists(p.name) {
            state::load(p.name)
                .map(|s| s.display_name)
                .unwrap_or_else(|_| p.box_name.to_string())
        } else {
            p.box_name.to_string()
        };
        let utm_status = live
            .iter()
            .find(|e| e.name == utm_name)
            .map(|e| e.status.as_str())
            .unwrap_or("—");
        println!(
            "{:<18} {:<10} {:<8} {:<8} {:<12} {}",
            p.name,
            format!("{:?}", p.os).to_lowercase(),
            p.ssh_port,
            format!("{} MiB", p.memory_mib),
            utm_status,
            utm_name,
        );
    }
    Ok(())
}

// ── vm build ──────────────────────────────────────────────────────────────────

/// Read the project's `src-tauri/Cargo.toml` (or `Cargo.toml` fallback) for the
/// package name, derive the VM-side binary path. Tries common target-dir
/// locations on the VM and returns the first that exists. Tauri ARM64 Linux
/// builds default to `aarch64-unknown-linux-gnu`; Windows VMs always emit
/// `x86_64-pc-windows-msvc` (see GAPS #1).
fn auto_detect_bin(profile: &profiles::VmProfile, session: &ssh2::Session) -> anyhow::Result<String> {
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

// ── vm screenshot ────────────────────────────────────────────────────────────

fn vm_screenshot(name: &str, out: &str) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    if profile.os != profiles::GuestOs::Linux {
        anyhow::bail!(
            "vm screenshot is Linux-only for now. \
             Windows VMs: connect via RDP at 127.0.0.1:{} (user vagrant / pass vagrant) \
             for visual access; programmatic capture from a headless SSH session needs \
             an active desktop session, which utm-dev doesn't currently provision.",
            profile.rdp_port.unwrap_or(3389),
        );
    }
    ssh::check(profile)?;
    let session = ssh::connect(profile)?;

    let remote = "/tmp/utm-dev-screenshot.png";
    println!("→ Capturing display :99 on {name}...");
    let (out_text, code) = ssh::exec_with_exit(
        &session,
        &format!("DISPLAY=:99 scrot --overwrite {remote} 2>&1 && ls -la {remote}"),
    )?;
    if code != 0 {
        anyhow::bail!(
            "screenshot failed (is `vm run` running with Xvfb on :99?):\n{out_text}"
        );
    }

    let local = std::path::PathBuf::from(out);
    println!("→ Pulling {} → {}", remote, local.display());
    ssh::download(profile, remote, &local)?;
    println!("✓ {}", local.display());
    Ok(())
}

// ── vm run ────────────────────────────────────────────────────────────────────

fn vm_run(name: &str, bin: Option<&str>) -> anyhow::Result<()> {
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

fn vm_build(name: &str, target: BuildTarget, _release: bool) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;

    // Windows ARM64 native isn't supported yet: VS Build Tools on ARM64
    // hosts ships only x64-targeting cross-tools (Hostarm64\x64), not
    // Hostarm64\arm64 — so the VM can't link aarch64-pc-windows-msvc
    // binaries. Builds on this VM go through x86_64 cross-compile (which
    // runs natively on ARM64 host producing x64 output, then runs under
    // x64 emulation if you launch the .exe on the same VM).
    if profile.os == profiles::GuestOs::Windows
        && (target == BuildTarget::Arm64 || target == BuildTarget::Both)
    {
        anyhow::bail!(
            "Windows ARM64 native build isn't currently supported — VS Build \
             Tools on ARM64 hosts doesn't ship a native ARM64-on-ARM64 \
             toolchain (Hostarm64\\arm64\\link.exe is missing). \
             Use --target x86-64 (the default for Windows is also x86-64)."
        );
    }

    // Linux x86_64 from ARM64 is now supported via Debian multiarch — see
    // build::ensure_linux_multiarch (called inside build::run when needed).

    // Auto-start VM if SSH not reachable
    if ssh::connect(profile).is_err() {
        println!("→ {} VM not reachable — starting it...", name);
        vm_up(name)?;
    }

    let project_dir = std::env::current_dir()
        .map_err(|e| anyhow::anyhow!("cannot determine current directory: {e}"))?;

    build::run(profile, &project_dir, target)
}

// ── vm delete ─────────────────────────────────────────────────────────────────

fn vm_delete(name: &str) -> anyhow::Result<()> {
    profiles::get(name)?; // validate name

    if !state::exists(name) {
        println!("✓ '{}' has no state — nothing to delete", name);
        return Ok(());
    }

    let st = state::load(name)?;

    // Stop VM if running
    let running = utm::list_vms()
        .unwrap_or_default()
        .into_iter()
        .any(|e| e.name == st.display_name && e.status == "started");
    if running {
        utm::stop_vm(&st.display_name)?;
    }

    // Try utmctl delete, fall back to AppleScript by UUID
    println!("→ Deleting '{}' from UTM...", st.display_name);
    let ok = std::process::Command::new(utm::UTMCTL)
        .args(["delete", &st.display_name])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let script = format!(
            r#"tell application "UTM" to delete virtual machine id "{}""#,
            st.uuid
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .status();
    }

    state::clear(name)?;
    println!("✓ Deleted '{}' ({})", st.display_name, name);
    Ok(())
}

// ── vm package ────────────────────────────────────────────────────────────────

fn vm_package(name: &str) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    let st = state::load(name)
        .map_err(|_| anyhow::anyhow!("'{}' not imported — run: utm-dev vm up --name {name}", name))?;

    // Stop VM if running
    let running = utm::list_vms()
        .unwrap_or_default()
        .into_iter()
        .any(|e| e.name == st.display_name && e.status == "started");
    if running {
        println!("→ Stopping VM before export...");
        utm::stop_vm(&st.display_name)?;
        std::thread::sleep(std::time::Duration::from_secs(8));
    }

    // Locate the .utm bundle UTM stores on disk
    let home   = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let bundle = home
        .join("Library/Containers/com.utmapp.UTM/Data/Documents")
        .join(format!("{}.utm", st.display_name));
    if !bundle.exists() {
        anyhow::bail!("VM bundle not found at {} — has UTM moved it?", bundle.display());
    }

    let bundle_gb = dir_size_bytes(&bundle)? as f64 / 1_073_741_824.0;
    println!("→ Packaging {} ({:.1} GB)...", bundle.display(), bundle_gb);

    // Output to <project>/.build/boxes/
    let project_dir = std::env::current_dir()?;
    let box_dir     = project_dir.join(".build").join("boxes");
    std::fs::create_dir_all(&box_dir)?;
    let box_file = box_dir.join(format!("{}-{name}_arm64.box", profile.box_name));

    // Build in a temp dir then tar
    let tmp_dir = std::env::temp_dir().join(format!("utm-dev-pkg-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)?;

    std::fs::write(
        tmp_dir.join("metadata.json"),
        r#"{"provider":"utm"}"#,
    )?;

    // cp -a bundle → tmp_dir/box.utm
    let dst = tmp_dir.join("box.utm");
    let cp_ok = std::process::Command::new("cp")
        .args(["-a", bundle.to_str().unwrap(), dst.to_str().unwrap()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !cp_ok {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        anyhow::bail!("Failed to copy VM bundle");
    }

    let tar_ok = std::process::Command::new("tar")
        .args([
            "-cf",
            box_file.to_str().unwrap(),
            "-C",
            tmp_dir.to_str().unwrap(),
            "metadata.json",
            "box.utm",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    if !tar_ok {
        anyhow::bail!("Failed to create .box archive");
    }

    let box_gb = std::fs::metadata(&box_file)?.len() as f64 / 1_073_741_824.0;
    println!("✓ Box: {} ({:.1} GB)", box_file.display(), box_gb);
    println!(
        "  To publish: vagrant cloud publish <username>/{}-{name} 1.0.0 utm {}",
        profile.box_name,
        box_file.display(),
    );
    Ok(())
}

fn dir_size_bytes(path: &std::path::Path) -> anyhow::Result<u64> {
    let output = std::process::Command::new("du")
        .args(["-sk", path.to_str().unwrap()])
        .output()?;
    let line  = String::from_utf8_lossy(&output.stdout);
    let kb: u64 = line.split_whitespace().next().unwrap_or("0").parse().unwrap_or(0);
    Ok(kb * 1024)
}
