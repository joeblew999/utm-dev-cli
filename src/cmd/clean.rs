/// `utm-dev clean` — free disk space across Rust targets, Xcode, Gradle, Bun caches, etc.
/// Mirrors the original utm-dev/.mise/tasks/clean/disk.ts.
///
/// Protected paths (NEVER touched):
///   ~/.cache/utm-dev                        — box images (~6 GB download)
///   ~/Library/Containers/com.utmapp.UTM     — your VMs
///   ~/.rustup/toolchains                    — Rust toolchains
///   ~/.android-sdk                          — Android SDK
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

const MIN_BYTES: u64 = 10_000_000;     // 10 MB threshold for misc caches
const MIN_RUST_TARGET: u64 = 50_000_000; // 50 MB threshold for target/

struct Target {
    label: String,
    bytes: u64,
    clean: Box<dyn FnOnce() -> Result<()>>,
}

pub fn run(deep: bool) -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let before = disk_free();

    println!("\n═══ utm-dev clean ═══");
    println!(
        "Disk: {} free of {} ({} used)",
        before.avail, before.total, before.pct
    );
    if deep {
        println!("Mode: deep clean");
    }
    println!();

    println!("Scanning...");
    let mut targets = scan(&home, deep)?;

    if targets.is_empty() {
        println!("\nNothing to clean (everything under threshold).");
        if !deep {
            println!("Try --deep for more aggressive cleanup.\n");
        }
        return Ok(());
    }

    targets.sort_by(|a, b| b.bytes.cmp(&a.bytes));

    let total: u64 = targets.iter().map(|t| t.bytes).sum();
    println!();
    println!("  {:>2}  {:<40} {}", "#", "What", "Size");
    println!("  {}", "─".repeat(55));
    for (i, t) in targets.iter().enumerate() {
        println!("  {:>2}  {:<40} {}", i + 1, t.label, fmt(t.bytes));
    }
    println!("  {}", "─".repeat(55));
    println!("      {:<40} ~{}", "TOTAL", fmt(total));
    println!();

    println!("Cleaning...\n");
    let mut cleaned: u64 = 0;
    for t in targets {
        print!("  {}...", t.label);
        let _ = std::io::Write::flush(&mut std::io::stdout());
        match (t.clean)() {
            Ok(()) => {
                println!(" {} freed", fmt(t.bytes));
                cleaned += t.bytes;
            }
            Err(e) => println!(" FAILED ({e})"),
        }
    }

    let after = disk_free();
    println!("\n═══ Done ═══");
    println!("Freed: ~{}", fmt(cleaned));
    println!(
        "Disk:  {} -> {} free ({} used)",
        before.avail, after.avail, after.pct
    );

    println!("\nProtected (never touched):");
    for (path, reason) in protected_paths(&home) {
        if path.exists() {
            let bytes = dir_bytes(&path);
            let short = path.display().to_string().replacen(
                home.to_str().unwrap_or(""),
                "~",
                1,
            );
            println!("  {} ({}) — {}", short, fmt(bytes), reason);
        }
    }

    if !deep {
        println!("\nTip: use --deep for Homebrew, Xcode archives, Docker, device support.");
    }
    println!();
    Ok(())
}

// ── Scan ─────────────────────────────────────────────────────────────────────

fn scan(home: &Path, deep: bool) -> Result<Vec<Target>> {
    let mut t: Vec<Target> = Vec::new();

    // 1. Rust target/ directories under common workspace dirs
    for dir in &["workspace", "src", "projects", "code"] {
        let root = home.join(dir);
        if !root.exists() {
            continue;
        }
        for target_dir in find_target_dirs(&root, 6) {
            // Confirm it's a Rust target (debug/, release/, or .rustc_info.json)
            let is_rust = target_dir.join("debug").exists()
                || target_dir.join("release").exists()
                || target_dir.join(".rustc_info.json").exists();
            if !is_rust {
                continue;
            }
            let bytes = dir_bytes(&target_dir);
            if bytes < MIN_RUST_TARGET {
                continue;
            }
            // Project label = last 2 path segments minus /target
            let parent = target_dir.parent().unwrap_or(&target_dir).to_path_buf();
            let label = parent
                .components()
                .rev()
                .take(2)
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let label = format!("Rust: {}", label.iter().rev().cloned().collect::<Vec<_>>().join("/"));
            let cargo_toml = parent.join("Cargo.toml");
            let target_path = target_dir.clone();
            t.push(Target {
                label,
                bytes,
                clean: Box::new(move || {
                    if cargo_toml.exists() {
                        let _ = Command::new("cargo")
                            .args(["clean", "--manifest-path"])
                            .arg(&cargo_toml)
                            .output();
                    } else {
                        let _ = std::fs::remove_dir_all(&target_path);
                    }
                    Ok(())
                }),
            });
        }
    }

    // 2. iOS unavailable simulators (macOS only)
    if cfg!(target_os = "macos") {
        if let Ok(out) = Command::new("xcrun")
            .args(["simctl", "list", "devices", "unavailable", "-j"])
            .output()
        {
            if out.status.success() {
                if let Ok(json) = String::from_utf8(out.stdout) {
                    let count = count_unavailable_devices(&json);
                    if count > 0 {
                        let est = count as u64 * 500_000_000;
                        t.push(Target {
                            label: format!("iOS simulators ({} unavailable)", count),
                            bytes: est,
                            clean: Box::new(|| {
                                let _ = Command::new("xcrun")
                                    .args(["simctl", "delete", "unavailable"])
                                    .output();
                                Ok(())
                            }),
                        });
                    }
                }
            }
        }
    }

    // 3-9. Standard cache directories
    add_cache(&mut t, home.join("Library/Developer/CoreSimulator/Caches"), "CoreSimulator caches");
    add_cache(&mut t, home.join("Library/Developer/Xcode/DerivedData"), "Xcode DerivedData");
    add_cache(&mut t, home.join(".cargo/registry/cache"), "Cargo registry cache");
    add_cache(&mut t, home.join(".gradle/caches"), "Gradle caches");
    add_cache(&mut t, home.join(".bun/install/cache"), "Bun install cache");
    add_cache_with(&mut t, home.join(".npm/_cacache"), "npm cache", || {
        let _ = Command::new("npm").args(["cache", "clean", "--force"]).output();
        Ok(())
    });
    add_cache(&mut t, home.join("Library/Caches/CocoaPods"), "CocoaPods cache");

    if deep {
        add_cache_with(&mut t, home.join("Library/Caches/Homebrew"), "Homebrew cache", || {
            let _ = Command::new("brew").args(["cleanup", "--prune=all"]).output();
            Ok(())
        });
        add_cache(&mut t, home.join("Library/Developer/Xcode/Archives"), "Xcode Archives");
        add_cache(&mut t, home.join("Library/Developer/Xcode/iOS DeviceSupport"), "Xcode iOS DeviceSupport");

        if Path::new("/var/run/docker.sock").exists() {
            t.push(Target {
                label: "Docker (unused images, build cache)".into(),
                bytes: 0,
                clean: Box::new(|| {
                    let _ = Command::new("docker").args(["system", "prune", "-af"]).output();
                    Ok(())
                }),
            });
        }
    }

    Ok(t)
}

fn add_cache(targets: &mut Vec<Target>, path: PathBuf, label: &str) {
    let bytes = dir_bytes(&path);
    if bytes < MIN_BYTES {
        return;
    }
    let p = path.clone();
    targets.push(Target {
        label: label.into(),
        bytes,
        clean: Box::new(move || {
            let _ = std::fs::remove_dir_all(&p);
            Ok(())
        }),
    });
}

fn add_cache_with<F>(targets: &mut Vec<Target>, path: PathBuf, label: &str, cleaner: F)
where
    F: FnOnce() -> Result<()> + 'static,
{
    let bytes = dir_bytes(&path);
    if bytes < MIN_BYTES {
        return;
    }
    targets.push(Target {
        label: label.into(),
        bytes,
        clean: Box::new(cleaner),
    });
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn find_target_dirs(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth >= max_depth {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) == Some("target") {
                out.push(path.clone());
                continue; // don't recurse into target/
            }
            // Skip noisy hidden dirs
            if path.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.') || n == "node_modules")
                .unwrap_or(false)
            {
                continue;
            }
            stack.push((path, depth + 1));
        }
    }
    out
}

fn dir_bytes(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    let out = match Command::new("du").args(["-sk"]).arg(path).output() {
        Ok(o) if o.status.success() => o,
        _ => return 0,
    };
    let s = String::from_utf8_lossy(&out.stdout);
    let kb: u64 = s.split_whitespace().next().unwrap_or("0").parse().unwrap_or(0);
    kb * 1024
}

fn fmt(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    const KB: u64 = 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.0} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

struct DiskFree {
    total: String,
    avail: String,
    pct:   String,
}

fn disk_free() -> DiskFree {
    let target = if cfg!(target_os = "macos") {
        "/System/Volumes/Data"
    } else {
        "/"
    };
    let out = Command::new("df").args(["-h"]).arg(target).output();
    let parts: Vec<String> = out
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().nth(1).map(|l| l.split_whitespace().map(String::from).collect()))
        .unwrap_or_default();
    DiskFree {
        total: parts.get(1).cloned().unwrap_or_else(|| "?".into()),
        avail: parts.get(3).cloned().unwrap_or_else(|| "?".into()),
        pct:   parts.get(4).cloned().unwrap_or_else(|| "?".into()),
    }
}

fn protected_paths(home: &Path) -> Vec<(PathBuf, &'static str)> {
    vec![
        (home.join(".cache/utm-dev"),                       "box images (~6 GB download)"),
        (home.join("Library/Containers/com.utmapp.UTM"),    "your VMs"),
        (home.join(".rustup/toolchains"),                   "Rust toolchains"),
        (home.join(".android-sdk"),                         "Android SDK"),
    ]
}

fn count_unavailable_devices(json: &str) -> usize {
    // Lightweight: count occurrences of `"udid":` inside the unavailable list.
    // The JSON is `{"devices":{"runtime1":[{...}, {...}], ...}}`.
    json.matches(r#""udid":"#).count()
}
