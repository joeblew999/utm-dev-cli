use which::which;

struct Check {
    name: &'static str,
    required: bool,
    hint: &'static str,
}

const CHECKS: &[Check] = &[
    Check { name: "mise",         required: true,  hint: "curl https://mise.run | sh" },
    Check { name: "cargo",        required: true,  hint: "rustup install stable" },
    Check { name: "cargo-tauri",  required: true,  hint: "cargo install tauri-cli" },
    Check { name: "bun",          required: true,  hint: "mise install bun" },
    Check { name: "xcodebuild",   required: false, hint: "install Xcode from App Store (macOS only)" },
    Check { name: "adb",          required: false, hint: "mise run setup (installs Android SDK)" },
    Check { name: "utmctl",       required: false, hint: "brew install --cask utm (needed for Windows/Linux VMs)" },
    Check { name: "wasm-pack",    required: false, hint: "mise install cargo:wasm-pack" },
];

pub fn run() -> anyhow::Result<()> {
    println!("═══ utm-dev doctor ═══\n");

    let mut ok = 0usize;
    let mut missing_required = 0usize;
    let mut missing_optional = 0usize;

    for check in CHECKS {
        match which(check.name) {
            Ok(path) => {
                println!("  ✓ {:<20} {}", check.name, path.display());
                ok += 1;
            }
            Err(_) => {
                let label = if check.required { "✗" } else { "?" };
                println!("  {label} {:<20} not found — {}", check.name, check.hint);
                if check.required {
                    missing_required += 1;
                } else {
                    missing_optional += 1;
                }
            }
        }
    }

    // UTM version + pinned-baseline check (informational, not a failure).
    if let Some(ver) = crate::vm::utm::installed_utm_version() {
        let baseline = crate::vm::utm::MIN_UTM_VERSION;
        if version_at_least(&ver, baseline) {
            println!("  ✓ UTM {} (≥ baseline {})", ver, baseline);
        } else {
            println!(
                "  ⚠ UTM {} is below baseline {} — run `brew upgrade --cask utm`",
                ver, baseline
            );
        }
    }

    println!();
    println!(
        "ok={ok} missing_required={missing_required} missing_optional={missing_optional}"
    );

    if missing_required > 0 {
        anyhow::bail!("{missing_required} required tool(s) missing");
    }

    Ok(())
}

fn version_at_least(a: &str, b: &str) -> bool {
    let parts = |s: &str| -> Vec<u32> {
        s.split('.').filter_map(|x| x.parse().ok()).collect()
    };
    let pa = parts(a);
    let pb = parts(b);
    for i in 0..pa.len().max(pb.len()) {
        let av = pa.get(i).copied().unwrap_or(0);
        let bv = pb.get(i).copied().unwrap_or(0);
        if av > bv { return true; }
        if av < bv { return false; }
    }
    true
}
