use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestOs {
    Windows,
    Linux,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapMode {
    Full,
    SshOnly,
}

#[derive(Debug, Clone)]
pub struct VmProfile {
    pub name:        &'static str,
    pub os:          GuestOs,
    pub box_name:    &'static str,
    pub ssh_port:    u16,
    pub rdp_port:    Option<u16>,
    pub winrm_port:  Option<u16>,
    pub user:        &'static str,
    pub pass:        &'static str,
    pub bootstrap:   BootstrapMode,
    pub memory_mib:  u32,
    pub cpu_cores:   u32,
}

const PROFILES: &[VmProfile] = &[
    VmProfile {
        name:       "windows-build",
        os:         GuestOs::Windows,
        box_name:   "windows-11",
        ssh_port:   2222,
        rdp_port:   Some(3389),
        winrm_port: Some(55985), // host port 55985 → guest 5985 in plat-windows
        user:       "vagrant",
        pass:       "vagrant",
        bootstrap:  BootstrapMode::Full,
        memory_mib: 12288,
        cpu_cores:  4,
        // Vagrant utm/windows-11 ships with ~26 GB. VS Build Tools alone
        // takes 5-6 GB; a Tauri build adds another ~6 GB. 80 GB is comfortable.
    },
    VmProfile {
        name:       "windows-test",
        os:         GuestOs::Windows,
        box_name:   "windows-11",
        ssh_port:   2322,
        rdp_port:   Some(3489),
        winrm_port: Some(6985),
        user:       "vagrant",
        pass:       "vagrant",
        bootstrap:  BootstrapMode::SshOnly,
        memory_mib: 4096,
        cpu_cores:  2,
    },
    VmProfile {
        name:       "linux-build",
        os:         GuestOs::Linux,
        box_name:   "ubuntu-24.04",
        ssh_port:   2422,
        rdp_port:   None,
        winrm_port: None,
        user:       "vagrant",
        pass:       "vagrant",
        bootstrap:  BootstrapMode::Full,
        memory_mib: 4096,
        cpu_cores:  4,
        // Vagrant utm/ubuntu-24.04 ships with ~19 GB. Vanilla Tauri ate
        // ~10 GB at peak; 40 GB gives headroom for bigger user projects.
    },
    VmProfile {
        name:       "linux-test",
        os:         GuestOs::Linux,
        box_name:   "ubuntu-24.04",
        ssh_port:   2522,
        rdp_port:   None,
        winrm_port: None,
        user:       "vagrant",
        pass:       "vagrant",
        bootstrap:  BootstrapMode::SshOnly,
        memory_mib: 2048,
        cpu_cores:  2,
    },
    VmProfile {
        name:       "linux-dev",
        os:         GuestOs::Linux,
        box_name:   "debian-12",
        ssh_port:   2622,
        rdp_port:   None,
        winrm_port: None,
        user:       "vagrant",
        pass:       "vagrant",
        bootstrap:  BootstrapMode::Full,
        memory_mib: 6144,
        cpu_cores:  4,
    },
];

pub fn get(name: &str) -> Result<&'static VmProfile> {
    PROFILES
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| {
            let available: Vec<&str> = PROFILES.iter().map(|p| p.name).collect();
            anyhow::anyhow!(
                "Unknown VM profile '{}'. Available: {}",
                name,
                available.join(", ")
            )
        })
}

pub fn list() -> impl Iterator<Item = &'static VmProfile> {
    PROFILES.iter()
}
