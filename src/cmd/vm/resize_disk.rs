//! `utm-dev vm resize-disk` — grow the VM's primary qcow2 disk and print
//! the in-guest one-liner needed to extend the partition. Uses qemu-img
//! from the standalone `qemu` Homebrew package (UTM only ships the dylib).

use crate::vm::{profiles, state, utm};

pub fn run(name: &str, plus_gb: u32) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    let st = state::load(name)
        .map_err(|_| anyhow::anyhow!("'{name}' not imported — run: utm-dev vm up --name {name}"))?;

    // VM must be stopped — resizing a running qcow2 corrupts it.
    let running = utm::list_vms()
        .unwrap_or_default()
        .into_iter()
        .any(|e| e.name == st.display_name && e.status == "started");
    if running {
        println!(
            "→ Stopping {} (must be off to resize disk)...",
            st.display_name
        );
        utm::stop_vm(&st.display_name)?;
        std::thread::sleep(std::time::Duration::from_secs(8));
    }

    // Locate the qcow2: ~/Library/.../Documents/<display>.utm/Data/<uuid>.qcow2
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let bundle = home
        .join("Library/Containers/com.utmapp.UTM/Data/Documents")
        .join(format!("{}.utm", st.display_name))
        .join("Data");
    let qcow2 = std::fs::read_dir(&bundle)?
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().map(|x| x == "qcow2").unwrap_or(false))
        .ok_or_else(|| anyhow::anyhow!("no .qcow2 found in {}", bundle.display()))?
        .path();
    println!("→ qcow2: {}", qcow2.display());

    // UTM bundles qemu-img only as a dylib, not a runnable CLI. Use the
    // standalone qemu Homebrew package instead. Auto-install if missing.
    let qemu_img = ensure_qemu_img()?;

    // Get current size (info JSON)
    let info = std::process::Command::new(&qemu_img)
        .args(["info", "--output=json"])
        .arg(&qcow2)
        .output()
        .map_err(|e| anyhow::anyhow!("qemu-img info: {e}"))?;
    if !info.status.success() {
        anyhow::bail!(
            "qemu-img info failed: {}",
            String::from_utf8_lossy(&info.stderr)
        );
    }
    let info_text = String::from_utf8_lossy(&info.stdout);
    let virtual_gb = info_text
        .split("\"virtual-size\":")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|b| b as f64 / 1_073_741_824.0);
    if let Some(gb) = virtual_gb {
        println!("  current virtual size: {:.1} GB", gb);
    }

    println!("→ Growing qcow2 by +{plus_gb}G...");
    let status = std::process::Command::new(qemu_img)
        .args(["resize"])
        .arg(&qcow2)
        .arg(format!("+{plus_gb}G"))
        .status()
        .map_err(|e| anyhow::anyhow!("qemu-img resize: {e}"))?;
    if !status.success() {
        anyhow::bail!("qemu-img resize failed");
    }
    println!("✓ qcow2 grown");

    println!(
        "→ Now: utm-dev vm up --name {name}\n\
         Then to extend the partition inside the guest:"
    );
    match profile.os {
        profiles::GuestOs::Windows => {
            println!(
                "    utm-dev vm exec --name {name} 'powershell -NoProfile -Command \"Resize-Partition -DriveLetter C -Size (Get-PartitionSupportedSize -DriveLetter C).SizeMax\"'"
            );
        }
        profiles::GuestOs::Linux => {
            println!(
                "    utm-dev vm exec --name {name} 'sudo growpart /dev/vda 1 && sudo resize2fs /dev/vda1'"
            );
        }
    }
    Ok(())
}

fn ensure_qemu_img() -> anyhow::Result<String> {
    if let Ok(p) = which::which("qemu-img") {
        return Ok(p.to_string_lossy().into_owned());
    }
    println!("→ qemu-img not found — installing qemu via brew (~50 MB, one-time)...");
    let r = std::process::Command::new("brew")
        .args(["install", "qemu"])
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .status()
        .map_err(|e| anyhow::anyhow!("brew not found or failed: {e}"))?;
    if !r.success() {
        anyhow::bail!("brew install qemu failed");
    }
    let p = which::which("qemu-img")
        .map_err(|_| anyhow::anyhow!("qemu-img still not on PATH after brew install"))?;
    println!("✓ qemu-img: {}", p.display());
    Ok(p.to_string_lossy().into_owned())
}
