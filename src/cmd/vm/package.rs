//! `utm-dev vm package` — export a VM as a Vagrant-format .box archive
//! for distribution. Stops the VM if running, copies the .utm bundle to
//! a temp dir alongside a `metadata.json`, then tars the pair into
//! `<project>/.build/boxes/<box>-<name>_arm64.box`.

use crate::vm::{profiles, state, utm};

pub fn run(name: &str) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    let st = state::load(name).map_err(|_| {
        anyhow::anyhow!("'{}' not imported — run: utm-dev vm up --name {name}", name)
    })?;

    // Stop VM if running
    let running = utm::list_vms()
        .unwrap_or_default()
        .into_iter()
        .any(|e| e.name == st.display_name && e.status == "started");
    if running {
        println!("→ Stopping VM before export...");
        utm::stop_vm(&st.display_name)?;
        std::thread::sleep(std::time::Duration::from_secs(8));
    }

    // Locate the .utm bundle UTM stores on disk
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let bundle = home
        .join("Library/Containers/com.utmapp.UTM/Data/Documents")
        .join(format!("{}.utm", st.display_name));
    if !bundle.exists() {
        anyhow::bail!(
            "VM bundle not found at {} — has UTM moved it?",
            bundle.display()
        );
    }

    let bundle_gb = dir_size_bytes(&bundle)? as f64 / 1_073_741_824.0;
    println!("→ Packaging {} ({:.1} GB)...", bundle.display(), bundle_gb);

    // Output to <project>/.build/boxes/
    let project_dir = std::env::current_dir()?;
    let box_dir = project_dir.join(".build").join("boxes");
    std::fs::create_dir_all(&box_dir)?;
    let box_file = box_dir.join(format!("{}-{name}_arm64.box", profile.box_name));

    // Build in a temp dir then tar
    let tmp_dir = std::env::temp_dir().join(format!("utm-dev-pkg-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)?;

    std::fs::write(tmp_dir.join("metadata.json"), r#"{"provider":"utm"}"#)?;

    // cp -a bundle → tmp_dir/box.utm
    let dst = tmp_dir.join("box.utm");
    let cp_ok = std::process::Command::new("cp")
        .args(["-a", bundle.to_str().unwrap(), dst.to_str().unwrap()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !cp_ok {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        anyhow::bail!("Failed to copy VM bundle");
    }

    let tar_ok = std::process::Command::new("tar")
        .args([
            "-cf",
            box_file.to_str().unwrap(),
            "-C",
            tmp_dir.to_str().unwrap(),
            "metadata.json",
            "box.utm",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    if !tar_ok {
        anyhow::bail!("Failed to create .box archive");
    }

    let box_gb = std::fs::metadata(&box_file)?.len() as f64 / 1_073_741_824.0;
    println!("✓ Box: {} ({:.1} GB)", box_file.display(), box_gb);
    println!(
        "  To publish: vagrant cloud publish <username>/{}-{name} 1.0.0 utm {}",
        profile.box_name,
        box_file.display(),
    );
    Ok(())
}

fn dir_size_bytes(path: &std::path::Path) -> anyhow::Result<u64> {
    let output = std::process::Command::new("du")
        .args(["-sk", path.to_str().unwrap()])
        .output()?;
    let line = String::from_utf8_lossy(&output.stdout);
    let kb: u64 = line
        .split_whitespace()
        .next()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    Ok(kb * 1024)
}
