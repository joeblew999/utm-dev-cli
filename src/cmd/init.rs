/// `utm-dev init` — append [tools] (and optionally Android [env]) to mise.toml.
/// Idempotent: detects existing utm-dev marker, or warns if [tools]/[env] already present.
use anyhow::{Result, bail};
use std::fs;
use std::io::Write;

const MARKER: &str = "# utm-dev tools";

/// Minimal block — just enough for VM-driven Tauri builds.
const BLOCK_MINIMAL: &str = r#"
# ── Added by: utm-dev init ───────────────────────────────────────────────────

# utm-dev tools — added by utm-dev init
[tools]
rust              = "stable"
"cargo:tauri-cli" = "2"
bun               = "latest"
"#;

/// Full block — adds Xcode/Ruby/Java host tools and Android [env] paths.
const BLOCK_ANDROID: &str = r#"
# ── Added by: utm-dev init --android ────────────────────────────────────────

# utm-dev tools — added by utm-dev init --android
[tools]
rust              = "stable"
"cargo:tauri-cli" = "2"
bun               = "latest"
xcodegen          = {version = "latest", os = ["macos"]}
ruby              = {version = "3.3",    os = ["macos"]}
java              = "temurin-17.0.18+8"

# utm-dev env — Android SDK paths (installed by utm-dev setup)
[env]
ANDROID_HOME = "{{env.HOME}}/.android-sdk"
NDK_HOME = "{{env.HOME}}/.android-sdk/ndk/27.2.12479018"
JAVA_HOME = "{{env.HOME}}/.local/share/mise/installs/java/temurin-17.0.18+8"
_.path = ["{{env.HOME}}/.android-sdk/platform-tools", "{{env.HOME}}/.android-sdk/emulator", "{{env.HOME}}/.android-sdk/cmdline-tools/latest/bin"]
"#;

pub fn run(android: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let mise_toml = cwd.join("mise.toml");

    if !mise_toml.exists() {
        bail!(
            "No mise.toml found in {} — create one first, or run from your project root.",
            cwd.display()
        );
    }

    let content = fs::read_to_string(&mise_toml)?;

    if content.contains(MARKER) {
        println!("✓ Already initialised");
        return Ok(());
    }

    let has_tools = content
        .lines()
        .any(|l| l.trim_start().starts_with("[tools]"));
    let has_env = content.lines().any(|l| l.trim_start().starts_with("[env]"));

    if has_tools || has_env {
        println!("⚠ Your mise.toml already has [tools] and/or [env] sections.");
        println!("  Add the following lines manually to your existing sections:\n");
        if has_tools {
            println!("  # In your [tools] section:");
            println!(r#"  rust              = "stable""#);
            println!(r#"  "cargo:tauri-cli" = "2""#);
            println!(r#"  bun               = "latest""#);
            if android {
                println!(r#"  xcodegen          = {{version = "latest", os = ["macos"]}}"#);
                println!(r#"  ruby              = {{version = "3.3",    os = ["macos"]}}"#);
                println!(r#"  java              = "temurin-17.0.18+8""#);
            }
            println!();
        }
        if has_env && android {
            println!("  # In your [env] section:");
            println!(r#"  ANDROID_HOME = "{{{{env.HOME}}}}/.android-sdk""#);
            println!(r#"  NDK_HOME = "{{{{env.HOME}}}}/.android-sdk/ndk/27.2.12479018""#);
            println!(
                r#"  JAVA_HOME = "{{{{env.HOME}}}}/.local/share/mise/installs/java/temurin-17.0.18+8""#
            );
            println!(
                r#"  _.path = ["{{{{env.HOME}}}}/.android-sdk/platform-tools", "{{{{env.HOME}}}}/.android-sdk/emulator", "{{{{env.HOME}}}}/.android-sdk/cmdline-tools/latest/bin"]"#
            );
            println!();
        }
        return Ok(());
    }

    let block = if android {
        BLOCK_ANDROID
    } else {
        BLOCK_MINIMAL
    };
    let mut f = fs::OpenOptions::new().append(true).open(&mise_toml)?;
    f.write_all(block.as_bytes())?;

    let label = if android {
        "[tools] and [env] (Android)"
    } else {
        "[tools] (minimal)"
    };
    println!("✓ Added {label} to {}", mise_toml.display());
    println!();
    println!("Next:");
    println!("  mise install         # Install tools");
    if android {
        println!("  utm-dev setup        # Install Android SDK + Xcode deps");
    } else {
        println!("  utm-dev windows build  # cross-platform build via VMs");
    }
    Ok(())
}
