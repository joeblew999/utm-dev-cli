/// UTM operations via utmctl and AppleScript.
/// All functions are macOS-only (UTM only runs on macOS).
use anyhow::{bail, Context, Result};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use super::profiles::VmProfile;

pub const UTMCTL: &str = "/Applications/UTM.app/Contents/MacOS/utmctl";

/// Minimum UTM version we've validated against. We don't auto-upgrade an
/// older install (could break running VMs), but we warn loudly so the user
/// knows to update. Bump this when a UTM release ships a fix or feature
/// utm-dev relies on.
pub const MIN_UTM_VERSION: &str = "4.6.5";

// ── UTM app lifecycle ────────────────────────────────────────────────────────

pub fn ensure_utm() -> Result<()> {
    let status = Command::new(UTMCTL).arg("list").output();
    if status.map(|o| o.status.success()).unwrap_or(false) {
        warn_if_outdated();
        return Ok(());
    }

    if !std::path::Path::new(UTMCTL).exists() {
        println!("→ Installing UTM via brew (cask utm)...");
        let r = Command::new("brew")
            .args(["install", "--cask", "utm"])
            .env("HOMEBREW_NO_AUTO_UPDATE", "1")
            .status()?;
        if !r.success() {
            bail!("UTM install failed");
        }
        if !std::path::Path::new(UTMCTL).exists() {
            bail!("UTM install failed — utmctl not found after brew install");
        }
        println!("✓ UTM installed");
    }

    println!("→ Launching UTM...");
    Command::new("open").args(["-g", "/Applications/UTM.app"]).status()?;

    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        thread::sleep(Duration::from_secs(1));
        if Command::new(UTMCTL).arg("list").output().map(|o| o.status.success()).unwrap_or(false) {
            println!("✓ UTM ready");
            warn_if_outdated();
            return Ok(());
        }
    }
    bail!("UTM did not become ready after 30s");
}

/// Read UTM's CFBundleShortVersionString from Info.plist and warn if older
/// than MIN_UTM_VERSION. Non-fatal — won't refuse to run.
pub fn installed_utm_version() -> Option<String> {
    let plist = "/Applications/UTM.app/Contents/Info.plist";
    let out = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleShortVersionString", plist])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn warn_if_outdated() {
    if let Some(ver) = installed_utm_version() {
        if version_less_than(&ver, MIN_UTM_VERSION) {
            eprintln!(
                "⚠ UTM {} is older than utm-dev's tested baseline ({}).\n  \
                 Some VM operations may fail. Update via:\n    \
                 brew upgrade --cask utm",
                ver, MIN_UTM_VERSION
            );
        }
    }
}

/// Naive semver-ish compare on dotted numeric versions. Returns true if a < b.
fn version_less_than(a: &str, b: &str) -> bool {
    let parts = |s: &str| -> Vec<u32> {
        s.split('.').filter_map(|x| x.parse().ok()).collect()
    };
    let pa = parts(a);
    let pb = parts(b);
    for i in 0..pa.len().max(pb.len()) {
        let av = pa.get(i).copied().unwrap_or(0);
        let bv = pb.get(i).copied().unwrap_or(0);
        if av < bv { return true; }
        if av > bv { return false; }
    }
    false
}

// ── VM list ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct VmEntry {
    pub uuid:   String,
    pub status: String,
    pub name:   String,
}

pub fn list_vms() -> Result<Vec<VmEntry>> {
    let out = Command::new(UTMCTL).arg("list").output()
        .context("running utmctl list")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let entries = stdout
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with("UUID"))
        .map(|l| {
            let mut parts = l.split_whitespace();
            let uuid   = parts.next().unwrap_or("").to_string();
            let status = parts.next().unwrap_or("").to_string();
            let name   = parts.collect::<Vec<_>>().join(" ");
            VmEntry { uuid, status, name }
        })
        .collect();
    Ok(entries)
}

#[allow(dead_code)]
pub fn find_vm_by_uuid(uuid: &str) -> Result<Option<VmEntry>> {
    Ok(list_vms()?.into_iter().find(|e| e.uuid == uuid))
}

// ── VM lifecycle ─────────────────────────────────────────────────────────────

pub fn start_vm(display_name: &str) -> Result<()> {
    // Already running?
    if let Some(e) = list_vms()?.into_iter().find(|e| e.name == display_name) {
        if e.status == "started" {
            println!("✓ {} already running", display_name);
            return Ok(());
        }
    }

    println!("→ Starting {}...", display_name);
    for attempt in 1..=3 {
        let r = Command::new(UTMCTL).args(["start", display_name]).status()?;
        if r.success() {
            println!("✓ VM started");
            return Ok(());
        }
        if attempt < 3 {
            println!("  retry {attempt}/3...");
            thread::sleep(Duration::from_secs(5));
        }
    }
    bail!("Failed to start {} after 3 attempts", display_name);
}

pub fn stop_vm(display_name: &str) -> Result<()> {
    let running = list_vms()?
        .into_iter()
        .any(|e| e.name == display_name && e.status == "started");
    if !running {
        return Ok(());
    }
    println!("→ Stopping {}...", display_name);
    Command::new(UTMCTL).args(["stop", display_name]).status()?;
    thread::sleep(Duration::from_secs(5));
    Ok(())
}

// ── Boot wait ────────────────────────────────────────────────────────────────

pub fn wait_for_boot(profile: &VmProfile, timeout_secs: u64) -> Result<()> {
    use super::profiles::GuestOs;
    match profile.os {
        GuestOs::Linux   => wait_for_ssh(profile, timeout_secs),
        GuestOs::Windows => wait_for_winrm(profile.winrm_port.unwrap(), timeout_secs),
    }
}

fn wait_for_winrm(port: u16, timeout_secs: u64) -> Result<()> {
    println!("→ Waiting for Windows to boot (up to {}m)...", timeout_secs / 60);
    let url = format!("http://127.0.0.1:{}/wsman", port);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut elapsed = 0u64;
    while Instant::now() < deadline {
        if client.get(&url).send().is_ok() {
            println!("✓ Windows ready ({}s)", elapsed);
            return Ok(());
        }
        thread::sleep(Duration::from_secs(5));
        elapsed += 5;
        if elapsed % 30 == 0 {
            println!("  still booting... ({}s)", elapsed);
        }
    }
    bail!("Timeout waiting for Windows WinRM ({}s)", timeout_secs);
}

fn wait_for_ssh(profile: &VmProfile, timeout_secs: u64) -> Result<()> {
    use super::ssh;
    println!("→ Waiting for Linux to boot (up to {}m)...", timeout_secs / 60);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut elapsed = 0u64;
    while Instant::now() < deadline {
        if let Ok(session) = ssh::connect(profile) {
            if ssh::exec(&session, "echo ok").map(|o| o.contains("ok")).unwrap_or(false) {
                println!("✓ Linux ready ({}s)", elapsed);
                return Ok(());
            }
        }
        thread::sleep(Duration::from_secs(5));
        elapsed += 5;
        if elapsed % 30 == 0 {
            println!("  still booting... ({}s)", elapsed);
        }
    }
    bail!("Timeout waiting for Linux SSH ({}s)", timeout_secs);
}

// ── Network configuration (AppleScript) ─────────────────────────────────────

pub fn configure_network(uuid: &str, profile: &VmProfile) -> Result<()> {
    use super::profiles::GuestOs;

    // Find emulated NIC index
    let find_script = format!(r#"
tell application "UTM"
  set vm to virtual machine id "{uuid}"
  set cfg to configuration of vm
  set nis to network interfaces of cfg
  repeat with ni in nis
    if mode of ni is emulated then
      return index of ni
    end if
  end repeat
  return -1
end tell
"#);

    let out = Command::new("osascript").arg("-e").arg(&find_script).output()?;
    let nic_index = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if nic_index == "-1" || nic_index.is_empty() {
        bail!("No emulated network interface found on VM");
    }

    println!("→ Configuring port forwards on NIC {}...", nic_index);

    // Build the full AppleScript for port-forward rules
    let idx: u32 = nic_index.parse().context("parsing NIC index")?;
    let mut rules = vec![
        format!("set newPortForward to {{protocol:\"TcPp\", guest address:\"\", guest port:\"22\", host address:\"127.0.0.1\", host port:\"{}\"}}",
            profile.ssh_port),
    ];
    if profile.os == GuestOs::Windows {
        if let Some(rdp) = profile.rdp_port {
            rules.push(format!(
                "set newPortForward to {{protocol:\"TcPp\", guest address:\"\", guest port:\"3389\", host address:\"127.0.0.1\", host port:\"{rdp}\"}}",
            ));
        }
        if let Some(winrm) = profile.winrm_port {
            rules.push(format!(
                "set newPortForward to {{protocol:\"TcPp\", guest address:\"\", guest port:\"5985\", host address:\"127.0.0.1\", host port:\"{winrm}\"}}",
            ));
        }
    }

    let set_rules: String = rules
        .iter()
        .map(|r| format!("
      {r}
      copy newPortForward to the end of portForwards"))
        .collect();

    let net_script = format!(r#"
tell application "UTM"
  set vm to virtual machine id "{uuid}"
  set config to configuration of vm
  set networkInterfaces to network interfaces of config
  repeat with anInterface in networkInterfaces
    if index of anInterface is {idx} then
      set portForwards to {{}}
      {set_rules}
      set port forwards of anInterface to portForwards
    end if
  end repeat
  update configuration of vm with config
end tell
"#);

    let r = Command::new("osascript").arg("-e").arg(&net_script).output()?;
    if !r.status.success() {
        bail!("Failed to configure port forwards: {}", String::from_utf8_lossy(&r.stderr));
    }

    let mut ports = vec![format!("SSH:{}", profile.ssh_port)];
    if let Some(rdp) = profile.rdp_port   { ports.push(format!("RDP:{rdp}")); }
    if let Some(w)   = profile.winrm_port { ports.push(format!("WinRM:{w}")); }
    println!("✓ Network: {}", ports.join(" "));
    Ok(())
}

pub fn configure_resources(uuid: &str, memory_mib: u32, cpu_cores: u32) -> Result<()> {
    println!("→ Setting VM resources: {} MiB RAM, {} CPU cores...", memory_mib, cpu_cores);
    let script = format!(r#"
tell application "UTM"
  set vm to virtual machine id "{uuid}"
  set cfg to configuration of vm
  set memory of cfg to {memory_mib}
  set cpu cores of cfg to {cpu_cores}
  update configuration of vm with cfg
end tell
"#);
    let r = Command::new("osascript").arg("-e").arg(&script).output()?;
    if !r.status.success() {
        println!("  ⚠ Could not set VM resources (non-fatal)");
    } else {
        println!("✓ Resources: {} MiB RAM, {} cores", memory_mib, cpu_cores);
    }
    Ok(())
}
