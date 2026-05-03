//! `utm-dev vm clean` — categorized disk cleanup inside a guest VM.
//!
//! Three modes, controlled by flags on the `Clean` subcommand:
//!   default       — transient caches (idempotent, safe)
//!   `--deep`      — also nuke cargo target/registry + mise installs
//!   `--aggressive` — also one-shot Windows tweaks (hibernation off,
//!                    CompactOS, VSS clear, pagefile to D:, event logs).
//!                    Frees the most space; some require reboot to apply.
//!
//! All scripts live in `scripts/clean/*.{ps1,sh}` (pulled in via
//! `include_str!`). Two markers: `__DEEP_BLOCK__` (extra deep-mode targets)
//! and `__ACTION__` (the actual delete pass — empty in dry-run mode).
//! Phase 1 (transient clean) runs as one PS/sh script. Phase 2 (aggressive
//! tweaks, Windows-only) runs as separate per-step calls so each step's
//! outcome is visible even if a later one fails.

use crate::vm::{profiles, ssh};

const SCRIPT_LINUX: &str = include_str!("../../../scripts/clean/linux/main.sh");
const SCRIPT_LINUX_DEEP: &str = include_str!("../../../scripts/clean/linux/deep.sh");
const SCRIPT_LINUX_ACTION: &str = include_str!("../../../scripts/clean/linux/action.sh");
const SCRIPT_WINDOWS: &str = include_str!("../../../scripts/clean/windows/main.ps1");
const SCRIPT_WINDOWS_DEEP: &str = include_str!("../../../scripts/clean/windows/deep.ps1");
const SCRIPT_WINDOWS_ACTION: &str = include_str!("../../../scripts/clean/windows/action.ps1");

const AGGRESSIVE_STEPS: &[(&str, &str)] = &[
    (
        "Hibernation: powercfg /h off",
        include_str!("../../../scripts/clean/windows/aggressive-hibernation.ps1"),
    ),
    (
        "VSS shadows: vssadmin delete shadows /all",
        include_str!("../../../scripts/clean/windows/aggressive-vss.ps1"),
    ),
    (
        "CompactOS: compress system files (slow)",
        include_str!("../../../scripts/clean/windows/aggressive-compactos.ps1"),
    ),
    (
        "Pagefile: move to D:\\pagefile.sys (reboot to apply)",
        include_str!("../../../scripts/clean/windows/aggressive-pagefile.ps1"),
    ),
    (
        "Event logs: wevtutil cl (skipping SSH/Security/System/Setup)",
        include_str!("../../../scripts/clean/windows/aggressive-event-logs.ps1"),
    ),
];

pub fn run(name: &str, deep: bool, aggressive: bool, dry_run: bool) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    ssh::check(profile)?;
    let session = ssh::connect(profile)?;

    if aggressive && profile.os != profiles::GuestOs::Windows {
        anyhow::bail!("--aggressive is Windows-only (Linux build VMs don't have these knobs)");
    }

    let mode = match (deep, aggressive, dry_run) {
        (_, _, true) => "dry-run (no changes)",
        (_, true, false) => "aggressive (one-shot Windows tweaks)",
        (true, false, false) => "deep (incl. cargo + mise caches)",
        _ => "default (keeps build caches)",
    };
    println!("→ vm clean on {name} — {mode}");

    match profile.os {
        profiles::GuestOs::Linux => {
            let (out, code) = ssh::exec_with_exit(&session, &linux_clean_script(deep, dry_run))?;
            println!("{out}");
            if code != 0 {
                eprintln!("(some cleanup steps may have failed — non-fatal)");
            }
        }
        profiles::GuestOs::Windows => {
            let (out, code) = ssh::exec_ps_windows(&session, &windows_clean_script(deep, dry_run))?;
            println!("{out}");
            if code != 0 {
                eprintln!("(some transient-clean steps may have failed — non-fatal)");
            }
        }
    }

    if aggressive && profile.os == profiles::GuestOs::Windows {
        run_aggressive_tweaks(&session, dry_run)?;
    }

    println!("✓ done");
    Ok(())
}

fn windows_clean_script(deep: bool, dry_run: bool) -> String {
    SCRIPT_WINDOWS
        .replace(
            "__DEEP_BLOCK__",
            if deep { SCRIPT_WINDOWS_DEEP } else { "" },
        )
        .replace(
            "__ACTION__",
            if dry_run { "" } else { SCRIPT_WINDOWS_ACTION },
        )
}

fn linux_clean_script(deep: bool, dry_run: bool) -> String {
    SCRIPT_LINUX
        .replace("__DEEP_BLOCK__", if deep { SCRIPT_LINUX_DEEP } else { "" })
        .replace("__ACTION__", if dry_run { "" } else { SCRIPT_LINUX_ACTION })
}

fn run_aggressive_tweaks(session: &ssh::Session, dry_run: bool) -> anyhow::Result<()> {
    println!();
    println!("--- Aggressive (one-shot Windows tweaks) ---");
    let before = ps_c_free_gb(session).unwrap_or(0.0);

    for (label, ps) in AGGRESSIVE_STEPS {
        if dry_run {
            println!("  [dry-run] {label}");
            continue;
        }
        print!("  {label}...");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        match ssh::exec_ps_windows(session, ps) {
            Ok((out, _code)) => {
                println!();
                for line in out.lines() {
                    if !line.trim().is_empty() {
                        println!("{line}");
                    }
                }
            }
            Err(e) => println!(" SSH error: {e}"),
        }
    }

    if !dry_run {
        let after = ps_c_free_gb(session).unwrap_or(0.0);
        println!();
        println!(
            "C: free: {before:.1} GB -> {after:.1} GB ({:+.1} GB)",
            after - before
        );
    }
    Ok(())
}

fn ps_c_free_gb(session: &ssh::Session) -> Option<f64> {
    let (out, code) = ssh::exec_ps_windows(session, "(Get-PSDrive C).Free / 1GB").ok()?;
    if code != 0 {
        return None;
    }
    out.trim().parse::<f64>().ok()
}
