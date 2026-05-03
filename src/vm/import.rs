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
/// Returns `(uuid, display_name)` — the display_name is what UTM actually
/// assigned. UTM uses the bundle's internal `config.plist` Name field, so we
/// rewrite it to `profile.name` before import (otherwise two profiles using
/// the same box, e.g. linux-test + linux-build on ubuntu-24.04, collide in
/// `~/Library/Containers/com.utmapp.UTM/Data/Documents/` and the second
/// import lands as a half-broken orphan).
pub fn ensure_imported(profile: &VmProfile) -> Result<(String, String)> {
    // Already imported under our profile name? Reuse it.
    if let Some(entry) = utm::list_vms()?
        .into_iter()
        .find(|e| e.name == profile.name)
    {
        println!("✓ {} already in UTM ({})", profile.name, entry.uuid);
        return Ok((entry.uuid, entry.name));
    }

    println!("→ {} not found in UTM — importing...", profile.name);
    let box_path = if let Some(url) = profile.prebaked_url {
        download_prebaked(profile, url)?
    } else {
        download_box(profile)?
    };
    let cached_bundle = extract_box(&box_path, profile)?;
    let prepared = prepare_bundle_for_import(&cached_bundle, profile)?;
    let result = import_utm_bundle(&prepared);
    // Best-effort cleanup of the prepared copy; UTM has its own copy now.
    let _ = std::fs::remove_dir_all(&prepared);
    result
}

// ── Bundle preparation: per-profile copy + plist Name rewrite ───────────────

fn prepare_bundle_for_import(cached: &Path, profile: &VmProfile) -> Result<PathBuf> {
    let imports = cache_dir()?.join("imports");
    std::fs::create_dir_all(&imports)?;
    let target = imports.join(format!("{}.utm", profile.name));

    // Wipe stale prepared copy from a prior failed run.
    let _ = std::fs::remove_dir_all(&target);

    println!("→ Preparing bundle for import (renaming to '{}')...", profile.name);
    let status = std::process::Command::new("cp")
        .args(["-a"])
        .arg(cached)
        .arg(&target)
        .status()
        .context("running cp -a to copy bundle")?;
    if !status.success() {
        bail!("cp -a {} -> {} failed", cached.display(), target.display());
    }

    rewrite_plist_name(&target.join("config.plist"), profile.name)?;
    Ok(target)
}

fn rewrite_plist_name(plist_path: &Path, new_name: &str) -> Result<()> {
    let xml = std::fs::read_to_string(plist_path)
        .with_context(|| format!("reading {}", plist_path.display()))?;

    // The bundle's config.plist has a single <key>Name</key> inside the
    // Information dict. Locate the matching <string>...</string> and replace.
    let key = "<key>Name</key>";
    let key_pos = xml
        .find(key)
        .with_context(|| format!("'<key>Name</key>' not found in {}", plist_path.display()))?;
    let after_key = &xml[key_pos + key.len()..];
    let str_open_rel = after_key
        .find("<string>")
        .context("expected <string> after Name key")?;
    let content_start = key_pos + key.len() + str_open_rel + "<string>".len();
    let str_close_rel = xml[content_start..]
        .find("</string>")
        .context("expected </string> closing Name value")?;
    let content_end = content_start + str_close_rel;

    let mut out = String::with_capacity(xml.len() + new_name.len());
    out.push_str(&xml[..content_start]);
    out.push_str(new_name);
    out.push_str(&xml[content_end..]);
    std::fs::write(plist_path, out)
        .with_context(|| format!("writing {}", plist_path.display()))?;
    Ok(())
}

// ── Download ─────────────────────────────────────────────────────────────────

fn cache_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot determine home directory")?;
    let dir = home.join(".cache").join("utm-dev");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Download a pre-baked .box from a direct URL (typically Cloudflare R2 or
/// GitHub Release). Skips the Vagrant Cloud version-discovery dance —
/// `prebaked_url` is the concrete artifact URL, not a registry lookup.
/// Cached at `~/.cache/utm-dev/<box_name>_prebaked_arm64.box` so re-imports
/// don't re-download. Use a versioned path in the URL to bust the cache.
fn download_prebaked(profile: &VmProfile, url: &str) -> Result<PathBuf> {
    let dest = cache_dir()?.join(format!("{}_prebaked_arm64.box", profile.box_name));
    if dest.exists() {
        println!("  (using cached pre-baked box {})", dest.display());
        return Ok(dest);
    }

    println!("→ Downloading pre-baked box for {}: {}", profile.name, url);
    let partial = dest.with_extension("box.partial");
    let resume_from = std::fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
    if resume_from > 0 {
        println!("  Resuming from {:.2} GB...", resume_from as f64 / 1_073_741_824.0);
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(7200)))
        .build()
        .into();
    let mut req = agent.get(url);
    if resume_from > 0 {
        req = req.header("Range", &format!("bytes={resume_from}-"));
    }
    let resp = req.call().with_context(|| format!("GET {url}"))?;
    let status_code = resp.status().as_u16();
    let resumed = status_code == 206;
    if !(200..300).contains(&status_code) {
        bail!("pre-baked box download failed: HTTP {status_code}");
    }
    if resume_from > 0 && !resumed {
        println!("  (server doesn't support resume — starting from 0)");
    }
    let starting_offset = if resumed { resume_from } else { 0 };
    let body_len: u64 = resp.headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let total = starting_offset + body_len;
    let mut body_reader = resp.into_body().into_reader();

    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] \
                 {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta})",
            )
            .unwrap()
            .progress_chars("=>-"),
    );
    pb.set_position(starting_offset);
    pb.enable_steady_tick(Duration::from_secs(5));

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!resumed)
        .append(resumed)
        .open(&partial)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = body_reader.read(&mut buf)?;
        if n == 0 { break; }
        file.write_all(&buf[..n])?;
        pb.inc(n as u64);
    }
    pb.finish_with_message("downloaded");

    if std::fs::metadata(&partial)?.len() < 100_000_000 {
        let _ = std::fs::remove_file(&partial);
        bail!("Pre-baked download too small (<100 MB) — likely failed");
    }
    std::fs::rename(&partial, &dest)?;
    Ok(dest)
}

fn download_box(profile: &VmProfile) -> Result<PathBuf> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();

    // Step 1: get latest version from Vagrant Cloud
    println!("→ Fetching box version for {}...", profile.box_name);
    let versions_url = format!("{VAGRANT_API}/box/{}/versions", profile.box_name);
    let versions_body = agent
        .get(&versions_url)
        .call()
        .with_context(|| format!("GET {versions_url}"))?
        .into_body()
        .read_to_string()?;
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
    let download_body = agent
        .get(&download_api)
        .call()
        .with_context(|| format!("GET {download_api}"))?
        .into_body()
        .read_to_string()?;
    let box_url = download_body
        .split("\"url\":\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .with_context(|| "cannot parse download URL from Vagrant Cloud response")?
        .to_string();

    // Step 4: download with progress (resumes if .partial exists)
    let is_windows = profile.os == super::profiles::GuestOs::Windows;
    let partial = dest.with_extension("box.partial");
    let resume_from = std::fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);

    if resume_from > 0 {
        println!(
            "→ Resuming download from {:.2} GB...",
            resume_from as f64 / 1_073_741_824.0
        );
    } else {
        println!(
            "→ Downloading box (~{}) — this takes a while...",
            if is_windows { "6 GB" } else { "1-2 GB" }
        );
    }

    let stream_agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(3600)))
        .build()
        .into();
    let mut req = stream_agent.get(&box_url);
    if resume_from > 0 {
        req = req.header("Range", &format!("bytes={resume_from}-"));
    }
    let resp = req.call().with_context(|| format!("GET {box_url}"))?;

    let status_code = resp.status().as_u16();
    let resumed = status_code == 206;
    if !(200..300).contains(&status_code) {
        bail!("Download failed: HTTP {status_code}");
    }
    if resume_from > 0 && !resumed {
        // Server didn't honour Range — start over
        println!("  (server doesn't support resume — starting from 0)");
    }
    let starting_offset = if resumed { resume_from } else { 0 };

    let body_len: u64 = resp.headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let total = starting_offset + body_len;
    let mut body_reader = resp.into_body().into_reader();
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] \
                 {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta})",
            )
            .unwrap()
            .progress_chars("=>-"),
    );
    pb.set_position(starting_offset);
    pb.enable_steady_tick(Duration::from_secs(5));

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!resumed)
        .append(resumed)
        .open(&partial)
        .with_context(|| format!("opening {}", partial.display()))?;

    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = body_reader.read(&mut buf)?;
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

    let compressed_size = std::fs::metadata(box_path)?.len();
    println!(
        "→ Extracting box ({:.1} GB compressed → ~{:.0} GB on disk)...",
        compressed_size as f64 / 1_073_741_824.0,
        compressed_size as f64 / 1_073_741_824.0 * 2.5,
    );

    let pb = ProgressBar::new(compressed_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] \
                 {bytes}/{total_bytes} read ({bytes_per_sec})",
            )
            .unwrap()
            .progress_chars("=>-"),
    );
    pb.enable_steady_tick(Duration::from_secs(5));

    let file = std::fs::File::open(box_path)
        .with_context(|| format!("opening {}", box_path.display()))?;
    let tracked = pb.wrap_read(file);
    let gz = flate2::read::GzDecoder::new(tracked);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(&dest_dir).context("extracting box archive")?;
    pb.finish_with_message("extracted");

    let bundle = find_utm_bundle(&dest_dir)?;
    println!("✓ Extracted: {}", bundle.display());
    Ok(bundle)
}

fn verify_imported_bundle(display_name: &str) -> bool {
    dirs::home_dir()
        .map(|home| {
            home.join("Library/Containers/com.utmapp.UTM/Data/Documents")
                .join(format!("{display_name}.utm"))
                .exists()
        })
        .unwrap_or(false)
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

fn import_utm_bundle(bundle: &Path) -> Result<(String, String)> {
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
                // Verify UTM actually placed the bundle on disk. If the move
                // collided (e.g. with a stale orphan), UTM may register a UUID
                // but leave it half-broken. Bail loudly so the user knows.
                if !verify_imported_bundle(&entry.name) {
                    let _ = std::process::Command::new(utm::UTMCTL)
                        .args(["delete", &entry.uuid])
                        .status();
                    bail!(
                        "UTM registered '{}' ({}) but its bundle is missing from \
                         ~/Library/Containers/com.utmapp.UTM/Data/Documents — \
                         likely a name collision. Cleaned up the orphan; please retry.",
                        entry.name, entry.uuid
                    );
                }
                println!("✓ Imported: {} ({})", entry.name, entry.uuid);
                return Ok((entry.uuid, entry.name));
            }
        }
    }

    bail!("Import succeeded but no new VM appeared in UTM after 30s")
}
