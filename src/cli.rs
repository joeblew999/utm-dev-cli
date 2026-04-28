use clap::{Parser, Subcommand};

use crate::cmd::{clean, doctor, init, platform, setup, vm};

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

    /// Generate all platform icons from app-icon.png
    Icon,

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
        Commands::Icon => todo!("icon — generate platform icons"),
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
