use clap::Subcommand;

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
    /// Build app in a VM (auto-starts if needed, syncs code, pulls artifacts)
    Build {
        #[arg(long, help = "VM profile name")]
        name: String,
        #[arg(long, help = "Optimised release build")]
        release: bool,
    },
    /// Run a command in a VM via SSH
    Exec {
        #[arg(long, help = "VM profile name")]
        name: String,
        /// Command to run (quoted as a single string)
        cmd: Vec<String>,
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
    /// List available VM profiles
    Ls,
}

pub fn run(cmd: VmCommands) -> anyhow::Result<()> {
    match cmd {
        VmCommands::Up { name }             => vm_up(&name),
        VmCommands::Down { name }           => vm_down(&name),
        VmCommands::Exec { name, cmd }      => vm_exec(&name, &cmd.join(" ")),
        VmCommands::Adopt { name, utm_name } => vm_adopt(&name, &utm_name),
        VmCommands::Ls                      => vm_ls(),
        VmCommands::Build { name, release } => vm_build(&name, release),
        VmCommands::Delete { name }         => vm_delete(&name),
        VmCommands::Package { name }        => vm_package(&name),
    }
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
        // First run: import (download + import box)
        println!("── First run for '{}' ─────────────────────────", name);
        let uuid = import::ensure_imported(profile)?;
        utm::configure_network(&uuid, profile)?;
        utm::configure_resources(&uuid, profile.memory_mib, profile.cpu_cores)?;
        let display = profile.box_name.to_string();
        state::save(
            name,
            &state::VmState {
                uuid:         uuid.clone(),
                display_name: display.clone(),
            },
        )?;
        (uuid, display)
    };

    // Start and wait for boot
    utm::start_vm(&utm_display)?;
    utm::wait_for_boot(profile, 300)?;

    // Bootstrap (idempotent — checks before each step)
    let session = ssh::connect(profile)?;
    bootstrap::run(profile, &session)?;

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

fn vm_build(name: &str, _release: bool) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;

    // Auto-start VM if SSH not reachable
    if ssh::connect(profile).is_err() {
        println!("→ {} VM not reachable — starting it...", name);
        vm_up(name)?;
    }

    let project_dir = std::env::current_dir()
        .map_err(|e| anyhow::anyhow!("cannot determine current directory: {e}"))?;

    build::run(profile, &project_dir)
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
    println!("  To publish: vagrant cloud publish joeblew999/{}-{name} 1.0.0 utm {}", profile.box_name, box_file.display());
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
