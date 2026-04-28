use clap::{Parser, Subcommand, ValueEnum};

use crate::cmd::{clean, doctor, init, platform, setup, vm};

/// Target architecture for VM builds.
///
/// `Both` produces native + cross artifacts. On Apple Silicon → Windows,
/// MSVC cross-tools handle x86_64 from an ARM64 VM. On Linux, `X8664` and
/// `Both` aren't yet supported (multi-arch system libs required for WebKit
/// GTK) — use a Linux x86_64 host or wait for a follow-up.
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum BuildTarget {
    /// Native ARM64 only
    #[value(name = "arm64")]
    Arm64,
    /// x86_64 cross-compile (Windows: works; Linux: not yet supported)
    #[value(name = "x86_64", alias = "x86-64", alias = "x64")]
    X8664,
    /// Both arm64 and x86_64
    #[value(name = "both")]
    Both,
}

impl Default for BuildTarget {
    fn default() -> Self { BuildTarget::Both }
}

#[derive(Parser)]
#[command(
    name = "utm-dev",
    version,
    about = "Cross-platform Tauri builds on Apple Silicon — VMs handled automatically",
    long_about = None,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Check what's installed and what's missing
    Doctor,

    /// Set up platform dev tools (macOS: Android SDK + Xcode deps; Linux: system libs)
    Setup,

    /// Add [tools] and [env] to your project's mise.toml (idempotent)
    Init,

    /// Free disk space (Rust targets, caches, simulators)
    Clean {
        #[arg(long, help = "Also clean Homebrew, Xcode archives, Docker")]
        deep: bool,
    },

    /// Platform build/dev commands
    #[command(subcommand)]
    Mac(platform::MacCommands),

    #[command(subcommand)]
    Ios(platform::IosCommands),

    #[command(subcommand)]
    Android(platform::AndroidCommands),

    #[command(subcommand)]
    Windows(platform::WindowsCommands),

    #[command(subcommand)]
    Linux(platform::LinuxCommands),

    /// Build for every platform
    #[command(name = "all")]
    All {
        #[command(subcommand)]
        cmd: platform::AllCommands,
    },

    /// VM management (auto-handled by platform commands — use directly only if needed)
    #[command(subcommand)]
    Vm(vm::VmCommands),
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Doctor => doctor::run(),
        Commands::Setup => setup::run(),
        Commands::Init => init::run(),
        Commands::Clean { deep } => clean::run(deep),
        Commands::Mac(cmd) => platform::run_mac(cmd),
        Commands::Ios(cmd) => platform::run_ios(cmd),
        Commands::Android(cmd) => platform::run_android(cmd),
        Commands::Windows(cmd) => platform::run_windows(cmd),
        Commands::Linux(cmd) => platform::run_linux(cmd),
        Commands::All { cmd } => platform::run_all(cmd),
        Commands::Vm(cmd) => vm::run(cmd),
    }
}
