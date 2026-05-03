use clap::{Parser, Subcommand, ValueEnum};

use crate::cmd::{clean, doctor, init, mcp, platform, screenshot, setup, validate, vm};

/// Target architecture for VM builds.
///
/// `Both` produces native + cross artifacts. On Apple Silicon → Windows,
/// MSVC cross-tools handle x86_64 from an ARM64 VM. On Linux, `X8664` and
/// `Both` aren't yet supported (multi-arch system libs required for WebKit
/// GTK) — use a Linux x86_64 host or wait for a follow-up.
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq, Default)]
pub enum BuildTarget {
    /// Native ARM64 only
    #[value(name = "arm64")]
    Arm64,
    /// x86_64 cross-compile (Windows: works; Linux: not yet supported)
    #[value(name = "x86_64", alias = "x86-64", alias = "x64")]
    X8664,
    /// Both arm64 and x86_64
    #[value(name = "both")]
    #[default]
    Both,
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

    /// Add [tools] (and optionally Android [env]) to your project's mise.toml
    Init {
        /// Include Android SDK paths and Java pin (otherwise minimal block)
        #[arg(long)]
        android: bool,
    },

    /// Free disk space (Rust targets, caches, simulators)
    Clean {
        #[arg(long, help = "Also clean Homebrew, Xcode archives, Docker")]
        deep: bool,
    },

    /// Set up MCP servers (.mcp.json + .claude/settings.json) for AI-assisted
    /// development. Idempotent — merges into existing JSON.
    Mcp,

    /// Capture the rendered Tauri WebView via WebDriver (host-side). Walks up
    /// from cwd to find src-tauri/. Requires `tauri-webdriver` on PATH.
    /// Different from `vm screenshot` (which is for Linux VMs and returns a
    /// black PNG for WebKit-GTK content).
    Screenshot {
        /// Output PNG path. Defaults to <project>/screenshots/app.png.
        #[arg(long)]
        out: Option<String>,
        /// WebDriver port to use. Defaults to 4444.
        #[arg(long)]
        port: Option<u16>,
    },

    /// Compare a screenshot against a golden PNG. Exits non-zero if the match
    /// percentage drops below `--tolerance` (default 95%). Writes a red/green
    /// diff PNG showing drift when validation fails. Pairs with `screenshot`
    /// for UI regression.
    Validate {
        /// Path to the actual screenshot (PNG)
        #[arg(long)]
        actual: String,
        /// Path to the golden template (PNG)
        #[arg(long)]
        golden: String,
        /// Match percentage required to pass (0–100)
        #[arg(long, default_value_t = 95.0)]
        tolerance: f64,
        /// Where to write the diff PNG when validation fails
        #[arg(long)]
        diff: Option<String>,
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
        Commands::Init { android } => init::run(android),
        Commands::Clean { deep } => clean::run(deep),
        Commands::Mcp => mcp::run(),
        Commands::Screenshot { out, port } => screenshot::run(out.as_deref(), port),
        Commands::Validate {
            actual,
            golden,
            tolerance,
            diff,
        } => validate::run(
            std::path::Path::new(&actual),
            std::path::Path::new(&golden),
            tolerance,
            diff.as_deref().map(std::path::Path::new),
        ),
        Commands::Mac(cmd) => platform::run_mac(cmd),
        Commands::Ios(cmd) => platform::run_ios(cmd),
        Commands::Android(cmd) => platform::run_android(cmd),
        Commands::Windows(cmd) => platform::run_windows(cmd),
        Commands::Linux(cmd) => platform::run_linux(cmd),
        Commands::All { cmd } => platform::run_all(cmd),
        Commands::Vm(cmd) => vm::run(cmd),
    }
}
