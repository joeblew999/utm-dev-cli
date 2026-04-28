use clap::Subcommand;

#[derive(Subcommand)]
pub enum VmCommands {
    /// Start a VM (imports box + bootstraps on first run, then just starts)
    Up {
        #[arg(long, help = "VM profile name (windows-11 | ubuntu-24.04 | debian-12)")]
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
        /// Command to run
        cmd: Vec<String>,
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
}

pub fn run(cmd: VmCommands) -> anyhow::Result<()> {
    match cmd {
        VmCommands::Up   { name }          => todo!("vm up --name {name}"),
        VmCommands::Down { name }          => todo!("vm down --name {name}"),
        VmCommands::Build { name, release } => todo!("vm build --name {name} (release={release})"),
        VmCommands::Exec { name, cmd }     => todo!("vm exec --name {name} -- {}", cmd.join(" ")),
        VmCommands::Delete { name }        => todo!("vm delete --name {name}"),
        VmCommands::Package { name }       => todo!("vm package --name {name}"),
    }
}
