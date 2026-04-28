use clap::Subcommand;

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
    /// Build for every platform
    Build,
}

pub fn run_mac(cmd: MacCommands) -> anyhow::Result<()> {
    match cmd {
        MacCommands::Dev   => todo!("mac dev"),
        MacCommands::Build => todo!("mac build"),
    }
}

pub fn run_ios(cmd: IosCommands) -> anyhow::Result<()> {
    match cmd {
        IosCommands::Sim   => todo!("ios sim"),
        IosCommands::Xcode => todo!("ios xcode"),
        IosCommands::Build => todo!("ios build"),
    }
}

pub fn run_android(cmd: AndroidCommands) -> anyhow::Result<()> {
    match cmd {
        AndroidCommands::Sim    => todo!("android sim"),
        AndroidCommands::Studio => todo!("android studio"),
        AndroidCommands::Build  => todo!("android build"),
    }
}

pub fn run_windows(cmd: WindowsCommands) -> anyhow::Result<()> {
    match cmd {
        WindowsCommands::Build { release } => todo!("windows build (release={release})"),
    }
}

pub fn run_linux(cmd: LinuxCommands) -> anyhow::Result<()> {
    match cmd {
        LinuxCommands::Dev          => todo!("linux dev"),
        LinuxCommands::Build { release } => todo!("linux build (release={release})"),
    }
}

pub fn run_all(cmd: AllCommands) -> anyhow::Result<()> {
    match cmd {
        AllCommands::Build => todo!("all build"),
    }
}
