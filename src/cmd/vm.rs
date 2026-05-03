use clap::Subcommand;

use crate::cli::BuildTarget;
use crate::vm::{bootstrap, build, import, profiles, ssh, state, utm};

mod clean;
mod debloat;
mod doctor;
mod package;
mod resize_disk;
mod run;

#[derive(Subcommand)]
pub enum VmCommands {
    /// Start a VM (imports box + bootstraps on first run, then just starts)
    Up {
        #[arg(
            long,
            help = "VM profile name (windows-build | linux-build | linux-dev | …)"
        )]
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
    /// Clean transient state on the VM: build/run logs, temp files,
    /// installer leftovers, Windows Update cache, VS Package Cache, DISM
    /// component store. Reports per-category sizes BEFORE cleaning.
    ///
    ///   default       — transient caches only (idempotent, safe)
    ///   --deep        — also nuke cargo target/registry + mise installs (rebuilds slow)
    ///   --aggressive  — also one-shot Windows tweaks (hibernation off, CompactOS,
    ///                   VSS clear, pagefile to D:, event logs). Frees the most
    ///                   space; some require reboot to fully apply.
    ///   --dry-run     — report only, no changes
    Clean {
        #[arg(long, help = "VM profile name")]
        name: String,
        /// Also nuke cargo target/registry caches and mise tool installs.
        /// They take longer to rebuild but free the most space.
        #[arg(long)]
        deep: bool,
        /// One-shot Windows tweaks to permanently shrink C: usage. Windows-only.
        /// Disables hibernation, runs compact /CompactOS, clears VSS shadows,
        /// moves pagefile to D:, empties event logs. Idempotent.
        #[arg(long)]
        aggressive: bool,
        /// Report category sizes only — no actual cleanup.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove pre-installed Windows Store apps that have no purpose on a
    /// build VM (Xbox, Bing News, Mail, Calendar, Solitaire, Cortana, etc.).
    /// Removes both the per-user package AND the provisioned package, so
    /// they don't come back. Idempotent — already-removed apps no-op.
    /// Windows-only.
    Debloat {
        #[arg(long, help = "VM profile name")]
        name: String,
        /// Report what would be removed; don't remove.
        #[arg(long)]
        dry_run: bool,
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
        #[arg(
            long,
            default_value = "screenshot.png",
            help = "Local path for the .png"
        )]
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
        to: String,
    },
    /// Copy file or directory from VM to host (scp -r)
    Pull {
        #[arg(long, help = "VM profile name")]
        name: String,
        #[arg(long, help = "Source path on VM")]
        from: String,
        #[arg(long, help = "Local destination path on host")]
        to: String,
    },
    /// Wire an existing UTM VM to a profile (skips download/import)
    Adopt {
        #[arg(
            long,
            help = "Profile name to assign (windows-build | linux-build | …)"
        )]
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
        VmCommands::Up { name } => vm_up(&name),
        VmCommands::Down { name } => vm_down(&name),
        VmCommands::Restart { name } => {
            vm_down(&name)?;
            vm_up(&name)
        }
        VmCommands::Clean {
            name,
            deep,
            aggressive,
            dry_run,
        } => clean::run(&name, deep, aggressive, dry_run),
        VmCommands::Debloat { name, dry_run } => debloat::run(&name, dry_run),
        VmCommands::Exec { name, cmd } => vm_exec(&name, &cmd.join(" ")),
        VmCommands::Shell { name } => vm_shell(&name),
        VmCommands::Logs {
            name,
            kind,
            follow,
            tail,
            errors,
        } => vm_logs(&name, &kind, follow, tail, errors),
        VmCommands::Doctor { name } => doctor::run(&name),
        VmCommands::Push { name, from, to } => vm_push(&name, &from, &to),
        VmCommands::Pull { name, from, to } => vm_pull(&name, &from, &to),
        VmCommands::Adopt { name, utm_name } => vm_adopt(&name, &utm_name),
        VmCommands::Ls => vm_ls(),
        VmCommands::Build {
            name,
            target,
            release,
        } => vm_build(&name, target, release),
        VmCommands::Run { name, bin } => run::run(&name, bin.as_deref()),
        VmCommands::Screenshot { name, out } => vm_screenshot(&name, &out),
        VmCommands::Delete { name } => vm_delete(&name),
        VmCommands::Package { name } => package::run(&name),
        VmCommands::ResizeDisk { name, plus_gb } => resize_disk::run(&name, plus_gb),
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
            uuid: entry.uuid.clone(),
            display_name: utm_name.to_string(),
        },
    )?;

    println!(
        "✓ Adopted: profile '{}' → UTM VM '{}' ({})",
        name, utm_name, entry.uuid
    );
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
                uuid: uuid.clone(),
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

    // log_path is a PS-array-literal-suffix for Windows (one or more
    // single-quoted paths, comma-joined). For Windows `run`, Start-Process
    // splits stdout (run.log) and stderr (run.log.err) — we surface both.
    // Linux's `vm run` redirects 2>&1 into run.log, so a single path suffices.
    let log_path = match (kind, &profile.os) {
        ("build", profiles::GuestOs::Linux) => "~/.utm-dev-build/build.log".to_string(),
        ("build", profiles::GuestOs::Windows) => {
            r"'%USERPROFILE%\.utm-dev-build\build.log'".to_string()
        }
        ("run", profiles::GuestOs::Linux) => "~/.utm-dev-run/run.log".to_string(),
        ("run", profiles::GuestOs::Windows) => {
            r"'%USERPROFILE%\.utm-dev-run\run.log','%USERPROFILE%\.utm-dev-run\run.log.err'"
                .to_string()
        }
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
                r#"powershell -NoProfile -Command "$paths = @({log_path}) | Where-Object {{ Test-Path $_ }}; \
                  if ($paths) {{ \
                    $hits = Get-Content $paths | Select-String -Pattern '^error[:[ ]|^error\[E[0-9]+\]|^FAILED|panic|fatal error|mise ERROR|unresolved external symbol|LNK[0-9]+|cannot find -l|linker .* not found' -Context 1,5 -CaseSensitive:$false; \
                    if ($hits) {{ $hits | ForEach-Object {{ $_.Context.PreContext + $_.Line + $_.Context.PostContext + '---' }} }} else {{ '(no errors found — try `vm logs --tail 200`)' }} \
                  }} else {{ '(no log yet)' }}""#
            ),
        }
    } else {
        match (follow, tail, &profile.os) {
            (true, _, profiles::GuestOs::Linux) => format!("tail -F {log_path} 2>/dev/null"),
            (false, Some(n), profiles::GuestOs::Linux) => {
                format!("tail -n {n} {log_path} 2>/dev/null || echo '(no log yet)'")
            }
            (false, None, profiles::GuestOs::Linux) => {
                format!("cat {log_path} 2>/dev/null || echo '(no log yet)'")
            }
            (true, _, profiles::GuestOs::Windows) => format!(
                // -Wait only takes one path. For run/Windows we follow the
                // first-existing of stdout|stderr; the user's typical case is
                // tailing run.log to watch a long-running process. Stderr
                // is visible without --follow via `vm logs --kind run`.
                r#"powershell -NoProfile -Command "$p = (@({log_path}) | Where-Object {{ Test-Path $_ }} | Select-Object -First 1); if ($p) {{ Get-Content $p -Wait -Tail 1000 }} else {{ '(no log yet)' }}""#
            ),
            (false, Some(n), profiles::GuestOs::Windows) => format!(
                r#"powershell -NoProfile -Command "$paths = @({log_path}) | Where-Object {{ Test-Path $_ }}; if ($paths) {{ Get-Content $paths -Tail {n} }} else {{ '(no log yet)' }}""#
            ),
            (false, None, profiles::GuestOs::Windows) => format!(
                r#"powershell -NoProfile -Command "$paths = @({log_path}) | Where-Object {{ Test-Path $_ }}; if ($paths) {{ Get-Content $paths }} else {{ '(no log yet)' }}""#
            ),
        }
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
        .args([
            "-p",
            &profile.ssh_port.to_string(),
            "-t",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "LogLevel=ERROR",
        ])
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
        .args([
            "-r",
            "-P",
            &profile.ssh_port.to_string(),
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "LogLevel=ERROR",
            src,
            dst,
        ])
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

    println!(
        "{:<18} {:<10} {:<8} {:<8} {:<12} UTM NAME",
        "NAME", "OS", "SSH", "RAM", "UTM STATUS"
    );
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
        anyhow::bail!("screenshot failed (is `vm run` running with Xvfb on :99?):\n{out_text}");
    }

    let local = std::path::PathBuf::from(out);
    println!("→ Pulling {} → {}", remote, local.display());
    ssh::download(profile, remote, &local)?;
    println!("✓ {}", local.display());
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
