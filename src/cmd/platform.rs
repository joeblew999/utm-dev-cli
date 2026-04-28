use anyhow::{bail, Result};
use clap::Subcommand;
use std::process::Command;

// Matches setup.ts constants
const NDK_VERSION:   &str = "27.2.12479018";
const JAVA_VERSION:  &str = "temurin-17.0.18+8";

// ── Subcommand enums ──────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum MacCommands {
    /// Run macOS desktop app with hot reload
    Dev,
    /// Build macOS .app and .dmg
    Build,
}

#[derive(Subcommand)]
pub enum IosCommands {
    /// Run on iOS simulator (no signing required)
    Sim,
    /// Open in Xcode for physical device debugging
    Xcode,
    /// Build iOS release IPA (requires signing)
    Build,
}

#[derive(Subcommand)]
pub enum AndroidCommands {
    /// Run on Android emulator
    Sim,
    /// Open in Android Studio
    Studio,
    /// Build Android APK and AAB
    Build,
}

#[derive(Subcommand)]
pub enum WindowsCommands {
    /// Build Windows .msi/.exe in VM (auto-starts VM on first run)
    Build {
        #[arg(long, help = "Optimised release build")]
        release: bool,
    },
}

#[derive(Subcommand)]
pub enum LinuxCommands {
    /// Start Linux desktop VM for dev/testing
    Dev,
    /// Build Linux .deb/.AppImage in VM (auto-starts VM on first run)
    Build {
        #[arg(long, help = "Optimised release build")]
        release: bool,
    },
}

#[derive(Subcommand)]
pub enum AllCommands {
    /// Build for every platform (mac → windows → linux → android → ios)
    Build,
}

// ── Runners ───────────────────────────────────────────────────────────────────

pub fn run_mac(cmd: MacCommands) -> Result<()> {
    match cmd {
        MacCommands::Dev   => tauri(&["dev"]),
        MacCommands::Build => tauri(&["build"]),
    }
}

pub fn run_ios(cmd: IosCommands) -> Result<()> {
    match cmd {
        IosCommands::Sim   => tauri(&["ios", "dev"]),
        IosCommands::Xcode => tauri(&["ios", "xcode-project"]),
        IosCommands::Build => tauri(&["ios", "build"]),
    }
}

pub fn run_android(cmd: AndroidCommands) -> Result<()> {
    match cmd {
        AndroidCommands::Sim    => tauri_android(&["android", "dev"]),
        AndroidCommands::Studio => tauri_android(&["android", "android-studio-project"]),
        AndroidCommands::Build  => tauri_android(&["android", "build"]),
    }
}

pub fn run_windows(cmd: WindowsCommands) -> Result<()> {
    match cmd {
        WindowsCommands::Build { release } => {
            super::vm::run(super::vm::VmCommands::Build {
                name:    "windows-build".to_string(),
                release,
            })
        }
    }
}

pub fn run_linux(cmd: LinuxCommands) -> Result<()> {
    match cmd {
        LinuxCommands::Dev => {
            // Start the linux-dev VM (GNOME desktop) and leave it running
            super::vm::run(super::vm::VmCommands::Up {
                name: "linux-dev".to_string(),
            })
        }
        LinuxCommands::Build { release } => {
            super::vm::run(super::vm::VmCommands::Build {
                name:    "linux-build".to_string(),
                release,
            })
        }
    }
}

pub fn run_all(cmd: AllCommands) -> Result<()> {
    match cmd {
        AllCommands::Build => {
            println!("═══ Building all platforms ═══");
            let steps: &[(&str, fn() -> Result<()>)] = &[
                ("mac",     || tauri(&["build"])),
                ("windows", || super::vm::run(super::vm::VmCommands::Build { name: "windows-build".to_string(), release: true })),
                ("linux",   || super::vm::run(super::vm::VmCommands::Build { name: "linux-build".to_string(),   release: true })),
                ("android", || tauri_android(&["android", "build"])),
                ("ios",     || tauri(&["ios", "build"])),
            ];
            let mut failed = Vec::new();
            for (name, step) in steps {
                println!("\n── {name} ──");
                if let Err(e) = step() {
                    println!("  ✗ {name} build failed: {e:#}");
                    failed.push(*name);
                } else {
                    println!("  ✓ {name} done");
                }
            }
            if !failed.is_empty() {
                bail!("Build failed for: {}", failed.join(", "));
            }
            println!("\n✓ All platforms built");
            Ok(())
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn tauri(args: &[&str]) -> Result<()> {
    let status = Command::new("cargo")
        .arg("tauri")
        .args(args)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to run cargo tauri: {e}"))?;
    if !status.success() {
        bail!("cargo tauri {} exited with {}", args.join(" "), status);
    }
    Ok(())
}

fn tauri_android(args: &[&str]) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("tauri").args(args);
    for (k, v) in android_env() {
        cmd.env(k, v);
    }
    let status = cmd.status()
        .map_err(|e| anyhow::anyhow!("failed to run cargo tauri: {e}"))?;
    if !status.success() {
        bail!("cargo tauri {} exited with {}", args.join(" "), status);
    }
    Ok(())
}

pub fn android_env() -> Vec<(String, String)> {
    let home = dirs::home_dir().unwrap_or_default();
    let android_home = std::env::var("ANDROID_HOME")
        .unwrap_or_else(|_| home.join(".android-sdk").to_string_lossy().into_owned());
    let ndk_home = std::env::var("NDK_HOME")
        .unwrap_or_else(|_| format!("{android_home}/ndk/{NDK_VERSION}"));
    let java_home = std::env::var("JAVA_HOME")
        .unwrap_or_else(|_| {
            home.join(format!(".local/share/mise/installs/java/{JAVA_VERSION}"))
                .to_string_lossy()
                .into_owned()
        });
    let path_extra = format!(
        "{android_home}/platform-tools:{android_home}/emulator:\
         {android_home}/cmdline-tools/latest/bin:{java_home}/bin"
    );
    let path = std::env::var("PATH")
        .map(|p| format!("{path_extra}:{p}"))
        .unwrap_or(path_extra);
    vec![
        ("ANDROID_HOME".into(), android_home),
        ("NDK_HOME".into(), ndk_home),
        ("JAVA_HOME".into(), java_home),
        ("PATH".into(), path),
    ]
}
