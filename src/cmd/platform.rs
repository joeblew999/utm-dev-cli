use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::path::PathBuf;
use std::process::Command;

use crate::cli::BuildTarget;
use crate::vm::build::{ProjectKind, cargo_package_name, detect_project_kind};

// Matches setup.ts constants
const NDK_VERSION: &str = "27.2.12479018";
const JAVA_VERSION: &str = "temurin-17.0.18+8";

// ── Subcommand enums ──────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum MacCommands {
    /// Run macOS desktop app with hot reload
    Dev,
    /// Build macOS binary natively (no VM — cargo runs on the host).
    /// Tauri projects produce .app/.dmg bundles; plain cargo projects
    /// produce a single binary at `.build/macos/<arch>/<name>`.
    Build {
        #[arg(long, value_enum, default_value_t = BuildTarget::Arm64,
              help = "Architecture: arm64 (default Apple Silicon) | x86-64 (Intel) | both")]
        target: BuildTarget,
        #[arg(long, help = "Optimised release build")]
        release: bool,
    },
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
        #[arg(long, value_enum, default_value_t = BuildTarget::X8664,
              help = "Architecture: x86-64 (default; arm64/both not yet supported on Windows)")]
        target: BuildTarget,
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
        #[arg(long, value_enum, default_value_t = BuildTarget::Arm64,
              help = "Architecture: arm64 | x86-64 | both (x86-64 not yet supported on Linux)")]
        target: BuildTarget,
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
        MacCommands::Dev => tauri(&["dev"]),
        MacCommands::Build { target, release } => mac_build(target, release),
    }
}

/// Native macOS build. No VM — cargo runs on the host. Mirrors the
/// `windows build` / `linux build` surface (same flags, same output dir
/// shape under `.build/macos/<arch>/`) so callers can treat all three
/// targets uniformly. Respects the project's `mise.toml`.
fn mac_build(target: BuildTarget, release: bool) -> Result<()> {
    let project_dir = std::env::current_dir().context("cwd")?;
    let kind = detect_project_kind(&project_dir);
    let use_mise = ensure_mise_installed(&project_dir)?;

    let kind_label = match kind {
        ProjectKind::Tauri => "Tauri",
        ProjectKind::Cargo => "cargo",
    };
    let mode_label = if release { "release" } else { "debug" };
    let via_label = if use_mise { "mise" } else { "cargo direct" };
    println!("→ Project kind: {kind_label} | mode: {mode_label} | via: {via_label}");

    let artifacts_dir = project_dir.join(".build").join("macos");
    std::fs::create_dir_all(&artifacts_dir).ok();

    for (arch, triple) in mac_triples(target) {
        println!("\n── Target: {triple} ──");
        match kind {
            ProjectKind::Tauri => {
                build_tauri_mac(&project_dir, triple, mode_label, release, use_mise)?
            }
            ProjectKind::Cargo => build_cargo_mac(
                &project_dir,
                &artifacts_dir,
                arch,
                triple,
                mode_label,
                release,
                use_mise,
            )?,
        }
    }
    Ok(())
}

/// `(arch_label, rust_triple)` pairs for each `BuildTarget`.
fn mac_triples(target: BuildTarget) -> Vec<(&'static str, &'static str)> {
    match target {
        BuildTarget::Arm64 => vec![("arm64", "aarch64-apple-darwin")],
        BuildTarget::X8664 => vec![("x86_64", "x86_64-apple-darwin")],
        BuildTarget::Both => vec![
            ("arm64", "aarch64-apple-darwin"),
            ("x86_64", "x86_64-apple-darwin"),
        ],
    }
}

/// Run `mise install` if the project has a `mise.toml` and mise is on PATH.
/// Returns whether subsequent cargo invocations should go through `mise exec`.
/// Idempotent — `mise install` no-ops when tools already match.
fn ensure_mise_installed(project_dir: &std::path::Path) -> Result<bool> {
    let has_mise_toml = ["mise.toml", ".mise.toml"]
        .iter()
        .any(|f| project_dir.join(f).exists());
    let mise_on_path = which::which("mise").is_ok();
    match (has_mise_toml, mise_on_path) {
        (true, true) => {
            println!("→ Running mise install (project mise.toml detected)...");
            let status = Command::new("mise")
                .arg("install")
                .current_dir(project_dir)
                .status()
                .map_err(|e| anyhow::anyhow!("failed to run mise install: {e}"))?;
            if !status.success() {
                bail!("mise install exited with {}", status);
            }
            Ok(true)
        }
        (true, false) => {
            println!(
                "⚠ project has mise.toml but `mise` is not on PATH — using host cargo directly."
            );
            Ok(false)
        }
        (false, _) => Ok(false),
    }
}

fn build_tauri_mac(
    project_dir: &std::path::Path,
    triple: &str,
    mode_label: &str,
    release: bool,
    use_mise: bool,
) -> Result<()> {
    let mut args: Vec<&str> = vec!["tauri", "build", "--target", triple];
    if !release {
        args.push("--debug");
    }
    run_cargo(&args, use_mise, project_dir)?;
    // Tauri's bundle is a rich tree (.app + .dmg + ...); leave it where
    // cargo put it and just point at the canonical location.
    let bundle = project_dir
        .join("src-tauri")
        .join("target")
        .join(triple)
        .join(mode_label)
        .join("bundle");
    if bundle.exists() {
        println!("  ✓ bundle: {}", bundle.display());
    }
    Ok(())
}

fn build_cargo_mac(
    project_dir: &std::path::Path,
    artifacts_dir: &std::path::Path,
    arch: &str,
    triple: &str,
    mode_label: &str,
    release: bool,
    use_mise: bool,
) -> Result<()> {
    let mut args: Vec<&str> = vec!["build", "--target", triple];
    if release {
        args.push("--release");
    }
    run_cargo(&args, use_mise, project_dir)?;

    let pkg_name = cargo_package_name(project_dir)?;
    let src: PathBuf = project_dir
        .join("target")
        .join(triple)
        .join(mode_label)
        .join(&pkg_name);
    if !src.exists() {
        bail!("expected binary not found: {}", src.display());
    }
    let arch_dir = artifacts_dir.join(arch);
    std::fs::create_dir_all(&arch_dir)?;
    let dst = arch_dir.join(&pkg_name);
    std::fs::copy(&src, &dst)
        .with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
    let size = std::fs::metadata(&dst)?.len();
    println!(
        "  ✓ {} ({:.1} MB)",
        dst.display(),
        size as f64 / 1_048_576.0
    );
    Ok(())
}

/// Run cargo, optionally through `mise exec --` so the target project's
/// pinned tools are honored. `cwd` is set to `project_dir` so cargo finds
/// the right Cargo.toml regardless of where utm-dev was invoked from.
fn run_cargo(args: &[&str], use_mise: bool, project_dir: &std::path::Path) -> Result<()> {
    let mut cmd = if use_mise {
        let mut c = Command::new("mise");
        c.arg("exec").arg("--").arg("cargo").args(args);
        c
    } else {
        let mut c = Command::new("cargo");
        c.args(args);
        c
    };
    cmd.current_dir(project_dir);
    let status = cmd.status().map_err(|e| {
        anyhow::anyhow!(
            "failed to spawn {}{}: {e}",
            if use_mise {
                "mise exec -- cargo "
            } else {
                "cargo "
            },
            args.join(" ")
        )
    })?;
    if !status.success() {
        bail!(
            "{}{} exited with {}",
            if use_mise {
                "mise exec -- cargo "
            } else {
                "cargo "
            },
            args.join(" "),
            status
        );
    }
    Ok(())
}

pub fn run_ios(cmd: IosCommands) -> Result<()> {
    match cmd {
        IosCommands::Sim => tauri(&["ios", "dev"]),
        IosCommands::Xcode => tauri(&["ios", "xcode-project"]),
        IosCommands::Build => tauri(&["ios", "build"]),
    }
}

pub fn run_android(cmd: AndroidCommands) -> Result<()> {
    match cmd {
        AndroidCommands::Sim => tauri_android(&["android", "dev"]),
        AndroidCommands::Studio => tauri_android(&["android", "android-studio-project"]),
        AndroidCommands::Build => tauri_android(&["android", "build"]),
    }
}

pub fn run_windows(cmd: WindowsCommands) -> Result<()> {
    match cmd {
        WindowsCommands::Build { target, release } => {
            super::vm::run(super::vm::VmCommands::Build {
                name: "windows-build".to_string(),
                target,
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
        LinuxCommands::Build { target, release } => super::vm::run(super::vm::VmCommands::Build {
            name: "linux-build".to_string(),
            target,
            release,
        }),
    }
}

pub fn run_all(cmd: AllCommands) -> Result<()> {
    match cmd {
        AllCommands::Build => {
            println!("═══ Building all platforms ═══");
            type Step = (&'static str, fn() -> Result<()>);
            let steps: &[Step] = &[
                ("mac", || tauri(&["build"])),
                ("windows", || {
                    super::vm::run(super::vm::VmCommands::Build {
                        name: "windows-build".to_string(),
                        target: BuildTarget::X8664,
                        release: true,
                    })
                }),
                ("linux", || {
                    super::vm::run(super::vm::VmCommands::Build {
                        name: "linux-build".to_string(),
                        target: BuildTarget::Arm64,
                        release: true,
                    })
                }),
                ("android", || tauri_android(&["android", "build"])),
                ("ios", || tauri(&["ios", "build"])),
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
    let status = cmd
        .status()
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
    let ndk_home =
        std::env::var("NDK_HOME").unwrap_or_else(|_| format!("{android_home}/ndk/{NDK_VERSION}"));
    let java_home = std::env::var("JAVA_HOME").unwrap_or_else(|_| {
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
