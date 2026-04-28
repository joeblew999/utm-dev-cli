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
    /// Tail logs from inside the VM (build, or runtime once vm run lands)
    Logs {
        #[arg(long, help = "VM profile name")]
        name: String,
        #[arg(long, default_value = "build", help = "Which log: build | run")]
        kind: String,
        #[arg(long, help = "Follow (tail -f) instead of dumping the full log")]
        follow: bool,
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
        VmCommands::Logs { name, kind, follow } => vm_logs(&name, &kind, follow),
        VmCommands::Push { name, from, to } => vm_push(&name, &from, &to),
        VmCommands::Pull { name, from, to } => vm_pull(&name, &from, &to),
        VmCommands::Adopt { name, utm_name } => vm_adopt(&name, &utm_name),
        VmCommands::Ls                      => vm_ls(),
        VmCommands::Build { name, target, release } => vm_build(&name, target, release),
        VmCommands::Run { name, bin }       => vm_run(&name, bin.as_deref()),
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

fn vm_logs(name: &str, kind: &str, follow: bool) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    ssh::check(profile)?;

    let log_path = match (kind, &profile.os) {
        ("build", profiles::GuestOs::Linux)   => "~/.utm-dev-build/build.log".to_string(),
        ("build", profiles::GuestOs::Windows) => r"%USERPROFILE%\.utm-dev-build\build.log".to_string(),
        ("run",   profiles::GuestOs::Linux)   => "~/.utm-dev-run/run.log".to_string(),
        ("run",   profiles::GuestOs::Windows) => r"%USERPROFILE%\.utm-dev-run\run.log".to_string(),
        _ => anyhow::bail!("unknown kind '{kind}' (expected: build | run)"),
    };

    let cmd = match (follow, &profile.os) {
        (true,  profiles::GuestOs::Linux)   => format!("tail -F {log_path} 2>/dev/null"),
        (false, profiles::GuestOs::Linux)   => format!("cat {log_path} 2>/dev/null || echo '(no log yet)'"),
        (true,  profiles::GuestOs::Windows) => format!(
            r#"powershell -NoProfile -Command "Get-Content '{log_path}' -Wait -Tail 1000""#
        ),
        (false, profiles::GuestOs::Windows) => format!(
            r#"powershell -NoProfile -Command "if (Test-Path '{log_path}') {{ Get-Content '{log_path}' }} else {{ '(no log yet)' }}""#
        ),
    };

    let code = ssh::exec_streaming(profile, &cmd)?;
    if code != 0 && !follow {
        std::process::exit(code);
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

// ── vm run ────────────────────────────────────────────────────────────────────

fn vm_run(name: &str, bin: Option<&str>) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    ssh::check(profile)?;

    let bin = bin.ok_or_else(|| anyhow::anyhow!(
        "vm run --bin <path> is required. Pass the path to the built binary on the VM \
         (e.g. for the demo: --bin src-tauri/target/x86_64-pc-windows-msvc/release/app.exe). \
         Auto-detection from .build/ will land in a follow-up."
    ))?;

    let session = ssh::connect(profile)?;

    println!("→ Launching {bin} in {name} (output → ~/.utm-dev-run/run.log)...");

    let cmd = match profile.os {
        profiles::GuestOs::Linux => format!(
            // xvfb-run gives the GUI app a virtual display so it boots
            // headlessly; nohup detaches so the SSH channel can close.
            r#"mkdir -p ~/.utm-dev-run && \
               nohup xvfb-run -a "{bin}" > ~/.utm-dev-run/run.log 2>&1 & \
               sleep 2 && echo "PID=$!" && jobs -p"#
        ),
        profiles::GuestOs::Windows => format!(
            // Start-Process detaches; redirect both streams to the run log.
            // The .err file is rolled into run.log on read by `vm logs`.
            r#"powershell -NoProfile -Command ^
              "if (-not (Test-Path '$env:USERPROFILE\.utm-dev-run')) {{ New-Item -ItemType Directory -Path '$env:USERPROFILE\.utm-dev-run' | Out-Null }}; ^
               $log = '$env:USERPROFILE\.utm-dev-run\run.log'; ^
               $err = '$env:USERPROFILE\.utm-dev-run\run.log.err'; ^
               $p = Start-Process -FilePath '{bin}' -RedirectStandardOutput $log -RedirectStandardError $err -PassThru; ^
               Write-Output ('PID=' + $p.Id)""#
        ),
    };

    let (out, code) = ssh::exec_with_exit(&session, &cmd)?;
    if code != 0 {
        anyhow::bail!("Failed to launch {bin}:\n{out}");
    }
    println!("{out}");
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
