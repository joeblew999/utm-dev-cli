/// Download a Vagrant/UTM box from the Vagrant Cloud `utm` registry and import it into UTM.
///
/// Boxes live at: https://app.vagrantup.com/utm/{box_name}
/// API:           https://api.cloud.hashicorp.com/vagrant/2022-09-30/registry/utm/box/{box_name}
///
/// Each box is a .tar.gz containing a .utm bundle. The bundle already has VirtIO drivers,
/// WinRM, and SSH pre-configured — bootstrap (ssh.rs / bootstrap.rs) runs AFTER import.
use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::profiles::VmProfile;
use super::utm;

const VAGRANT_API: &str = "https://api.cloud.hashicorp.com/vagrant/2022-09-30/registry/utm";

/// Ensure the VM is imported into UTM, downloading and importing if needed.
/// Returns the UUID of the (possibly newly imported) VM.
pub fn ensure_imported(profile: &VmProfile) -> Result<String> {
    if let Some(entry) = utm::list_vms()?
        .into_iter()
        .find(|e| e.name == profile.box_name)
    {
        println!("✓ {} already in UTM ({})", profile.box_name, entry.uuid);
        return Ok(entry.uuid);
    }

    println!("→ {} not found in UTM — importing...", profile.box_name);
    let box_path = download_box(profile)?;
    let utm_bundle = extract_box(&box_path, profile)?;
    let uuid = import_utm_bundle(&utm_bundle)?;
    Ok(uuid)
}

// ── Download ─────────────────────────────────────────────────────────────────

fn cache_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot determine home directory")?;
    let dir = home.join(".cache").join("utm-dev");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn download_box(profile: &VmProfile) -> Result<PathBuf> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    // Step 1: get latest version from Vagrant Cloud
    println!("→ Fetching box version for {}...", profile.box_name);
    let versions_url = format!("{VAGRANT_API}/box/{}/versions", profile.box_name);
    let versions_body = client
        .get(&versions_url)
        .send()
        .with_context(|| format!("GET {versions_url}"))?
        .text()?;
    let box_version = versions_body
        .split("\"name\":\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .with_context(|| "cannot parse box version from Vagrant Cloud response")?
        .to_string();
    println!("  box version: {box_version}");

    // Step 2: check cache
    let dest = cache_dir()?.join(format!("{}_{}_arm64.box", profile.box_name, box_version));
    if dest.exists() {
        println!("  (using cached box {})", dest.display());
        return Ok(dest);
    }

    // Remove stale versions of this box
    if let Ok(entries) = std::fs::read_dir(cache_dir()?) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(profile.box_name) && name.ends_with(".box") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    // Step 3: get download URL
    let download_api = format!(
        "{VAGRANT_API}/box/{}/version/{box_version}/provider/utm/architecture/arm64/download",
        profile.box_name
    );
    let download_body = client
        .get(&download_api)
        .send()
        .with_context(|| format!("GET {download_api}"))?
        .text()?;
    let box_url = download_body
        .split("\"url\":\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .with_context(|| "cannot parse download URL from Vagrant Cloud response")?
        .to_string();

    // Step 4: download with progress
    let is_windows = profile.os == super::profiles::GuestOs::Windows;
    println!(
        "→ Downloading box (~{}) — this takes a while...",
        if is_windows { "6 GB" } else { "1-2 GB" }
    );

    let stream_client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3600))
        .build()?;
    let mut resp = stream_client
        .get(&box_url)
        .send()
        .with_context(|| format!("GET {box_url}"))?;
    if !resp.status().is_success() {
        bail!("Download failed: HTTP {}", resp.status());
    }

    let total = resp.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] \
                 {bytes}/{total_bytes} ({eta})",
            )
            .unwrap()
            .progress_chars("=>-"),
    );

    let partial = dest.with_extension("box.partial");
    let mut file =
        std::fs::File::create(&partial).with_context(|| format!("creating {}", partial.display()))?;

    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        pb.inc(n as u64);
    }
    pb.finish_with_message("downloaded");

    let min_size: u64 = if is_windows { 1_000_000_000 } else { 100_000_000 };
    if std::fs::metadata(&partial)?.len() < min_size {
        let _ = std::fs::remove_file(&partial);
        bail!("Downloaded file too small — likely a failed/partial download");
    }

    std::fs::rename(&partial, &dest)
        .with_context(|| format!("renaming {} → {}", partial.display(), dest.display()))?;

    Ok(dest)
}

// ── Extract ──────────────────────────────────────────────────────────────────

fn extract_box(box_path: &Path, profile: &VmProfile) -> Result<PathBuf> {
    let dest_dir = cache_dir()?.join(format!("{}-extracted", profile.box_name));
    if dest_dir.exists() {
        if let Ok(bundle) = find_utm_bundle(&dest_dir) {
            println!("  (using cached extraction {})", bundle.display());
            return Ok(bundle);
        }
        let _ = std::fs::remove_dir_all(&dest_dir);
    }
    std::fs::create_dir_all(&dest_dir)?;

    println!("→ Extracting box...");
    let file = std::fs::File::open(box_path)
        .with_context(|| format!("opening {}", box_path.display()))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(&dest_dir).context("extracting box archive")?;

    let bundle = find_utm_bundle(&dest_dir)?;
    println!("✓ Extracted: {}", bundle.display());
    Ok(bundle)
}

fn find_utm_bundle(dir: &Path) -> Result<PathBuf> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() && path.extension().map(|e| e == "utm").unwrap_or(false) {
            return Ok(path);
        }
        if path.is_dir() {
            if let Ok(inner) = find_utm_bundle(&path) {
                return Ok(inner);
            }
        }
    }
    bail!("No .utm bundle found in extracted box at {}", dir.display())
}

// ── UTM import ───────────────────────────────────────────────────────────────

fn import_utm_bundle(bundle: &Path) -> Result<String> {
    println!("→ Importing {} into UTM...", bundle.display());

    let bundle_str = bundle.to_str().context("bundle path not valid UTF-8")?;

    // Snapshot existing UUIDs so we can find the newly imported one
    let before: std::collections::HashSet<String> = utm::list_vms()?
        .into_iter()
        .map(|e| e.uuid)
        .collect();

    // Use the correct UTM AppleScript: "import new virtual machine from POSIX file"
    let script = format!(
        r#"tell application "UTM" to import new virtual machine from POSIX file "{bundle_str}""#
    );
    let out = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .context("running osascript to import VM")?;

    if !out.status.success() {
        bail!(
            "UTM import failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // Wait for the new VM to appear (up to 30s)
    println!("→ Waiting for UTM to register the imported VM...");
    for _ in 0..15 {
        std::thread::sleep(Duration::from_secs(2));
        for entry in utm::list_vms()? {
            if !before.contains(&entry.uuid) {
                println!("✓ Imported: {} ({})", entry.name, entry.uuid);
                return Ok(entry.uuid);
            }
        }
    }

    bail!("Import succeeded but no new VM appeared in UTM after 30s")
}
