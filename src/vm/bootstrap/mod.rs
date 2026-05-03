//! First-run bootstrap: installs build tools, Rust, and mise in a fresh VM.
//! Idempotent — safe to run multiple times (checks before installing).
//!
//! Two implementations, dispatched by guest OS:
//!   - [`linux`] — apt-driven, ssh transport
//!   - [`windows`] — WinRM transport, PowerShell-driven
//!
//! Shared helpers ([`find_public_key`]) live here.

use anyhow::{Context, Result, bail};

use crate::vm::profiles::{GuestOs, VmProfile};
use crate::vm::ssh;

mod linux;
mod windows;

pub fn run(profile: &VmProfile) -> Result<()> {
    match profile.os {
        GuestOs::Linux => {
            // Linux is reachable via SSH right after wait_for_boot succeeds.
            let session = ssh::connect(profile)?;
            linux::run(&session, profile)
        }
        GuestOs::Windows => windows::run(profile),
    }
}

/// Read the host's SSH public key from `~/.ssh/`. Used by both bootstrap
/// paths to authorise passwordless ssh from the host into the VM.
pub(in crate::vm::bootstrap) fn find_public_key() -> Result<String> {
    let home = dirs::home_dir().context("no home dir")?;
    for name in &["id_ed25519.pub", "id_rsa.pub", "id_ecdsa.pub"] {
        let path = home.join(".ssh").join(name);
        if path.exists() {
            return std::fs::read_to_string(&path)
                .map(|s| s.trim().to_string())
                .with_context(|| format!("reading {}", path.display()));
        }
    }
    bail!("No SSH public key found in ~/.ssh/ — generate one with: ssh-keygen -t ed25519")
}
