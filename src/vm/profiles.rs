use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestOs {
    Windows,
    Linux,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapMode {
    Full,
    SshOnly,
    None,
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
    /// Minimum disk size in GiB. If the imported qcow2 is smaller, vm up
    /// grows it (qemu-img resize) and bootstrap extends the guest partition
    /// (Resize-Partition / growpart + resize2fs). None = leave default.
    pub disk_gib:    Option<u32>,
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
        disk_gib:   Some(80),
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
        disk_gib:   None,
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
        disk_gib:   Some(40),
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
        disk_gib:   None,
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
        disk_gib:   Some(40),
    },
];

#[allow(dead_code)]
pub const DEFAULT_VM: &str = "windows-build";

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

#[allow(dead_code)]
pub fn vm_home(profile: &VmProfile) -> String {
    match profile.os {
        GuestOs::Windows => format!("C:\\Users\\{}", profile.user),
        GuestOs::Linux   => format!("/home/{}", profile.user),
    }
}

#[allow(dead_code)]
pub fn path_sep(profile: &VmProfile) -> char {
    match profile.os {
        GuestOs::Windows => '\\',
        GuestOs::Linux   => '/',
    }
}
