/// `utm-dev setup` — install dev tools (platform-aware).
///
/// macOS: Rust + Xcode check, Android SDK (cmdline-tools, platform, build-tools,
///   NDK, emulator, system image, AVD), Rust Android targets, CocoaPods.
/// Linux: apt-installed system C libraries Tauri links against, then `mise install`.
///
/// Idempotent — checks before each step.
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

const JAVA_VERSION:        &str = "temurin-17.0.18+8";
const NDK_VERSION:         &str = "27.2.12479018";
const BUILD_TOOLS_VERSION: &str = "35.0.0";
const PLATFORM_VERSION:    &str = "android-35";
const CMDLINE_TOOLS_URL:   &str =
    "https://dl.google.com/android/repository/commandlinetools-mac-14742923_latest.zip";
const AVD_NAME:            &str = "utm-dev";

pub fn run() -> Result<()> {
    println!("═══ utm-dev setup ═══");

    if cfg!(target_os = "macos") {
        setup_macos()
    } else if cfg!(target_os = "linux") {
        // Linux setup here is for a Linux *host* (e.g. for the linux-dev
        // case where someone is doing GUI work on the Linux VM directly).
        // utm-dev VM orchestration itself requires macOS — UTM doesn't run
        // anywhere else.
        setup_linux()
    } else {
        bail!(
            "utm-dev setup is supported on macOS (full) and Linux (host deps only). \
             VM orchestration commands (vm up/build/...) require macOS — UTM doesn't \
             run on Windows or Linux hosts."
        )
    }
}

// ── Linux ────────────────────────────────────────────────────────────────────

fn setup_linux() -> Result<()> {
    println!("  Platform: Linux\n");
    println!("── Stage 1: System C libraries (apt) ──");
    println!("  (Rust, bun, cargo-tauri are managed by mise [tools])\n");

    let webkit_installed = run_capture(
        "dpkg",
        &["-s", "libwebkit2gtk-4.1-dev"],
        None,
    )
    .map(|out| out.status.success())
    .unwrap_or(false);

    if webkit_installed {
        println!("  ✓ Tauri system deps already installed");
    } else {
        println!("  Installing system dependencies (requires sudo)...");
        let env = vec![("DEBIAN_FRONTEND", "noninteractive")];
        sudo_apt(&env, &["update", "-qq"])?;
        sudo_apt(&env, &[
            "install", "-y", "-qq",
            "build-essential", "curl", "git", "pkg-config",
            "libwebkit2gtk-4.1-dev", "libgtk-3-dev",
            "libjavascriptcoregtk-4.1-dev", "libsoup-3.0-dev",
            "libayatana-appindicator3-dev", "librsvg2-dev",
            "libssl-dev", "libxdo-dev",
            "patchelf", "wget", "file",
        ])?;
        println!("  ✓ System deps installed");
    }

    println!("\n── Stage 2: mise-managed tools ──");
    println!("  Running mise install...");
    let mise_status = Command::new("mise").arg("install").status();
    match mise_status {
        Ok(s) if s.success() => println!("  ✓ mise tools installed"),
        _ => println!("  ⚠ mise install had issues — run `mise install` manually to debug"),
    }

    if cmd_exists("cargo") {
        let ver = capture_first_word("cargo", &["--version"]);
        println!("  ✓ Rust {} (mise-managed)", ver);
    }
    if cmd_exists("bun") {
        let ver = capture_first_word("bun", &["--version"]);
        println!("  ✓ bun {} (mise-managed)", ver);
    }

    println!("\n═══ Setup complete (Linux) ═══");
    println!("\nNext:");
    println!("  cargo tauri dev      # Run desktop app (hot reload)");
    println!("  cargo tauri build    # Build .deb / .AppImage");
    println!("  utm-dev doctor       # Check everything");
    Ok(())
}

fn sudo_apt(env: &[(&str, &str)], args: &[&str]) -> Result<()> {
    let mut cmd = Command::new("sudo");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.arg("apt-get").args(args);
    let status = cmd.status().context("running sudo apt-get")?;
    if !status.success() {
        bail!("apt-get {:?} exited {}", args, status);
    }
    Ok(())
}

// ── macOS ────────────────────────────────────────────────────────────────────

fn setup_macos() -> Result<()> {
    println!("  Platform: macOS\n");

    // Stage 1: Host tools
    println!("── Stage 1: Host tools ──");
    if cmd_exists("cargo") {
        let ver = capture_first_word("cargo", &["--version"]);
        println!("  ✓ Rust {}", ver);
    } else {
        println!("  Installing Rust...");
        sh("curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y")?;
        println!("  ✓ Rust installed");
    }

    let xcode = run_capture("xcode-select", &["-p"], None)?;
    if !xcode.status.success() {
        bail!(
            "Xcode not found.\n  Install from: https://apps.apple.com/app/xcode/id497799835\n  \
             Then run: sudo xcode-select --switch /Applications/Xcode.app"
        );
    }
    println!(
        "  ✓ Xcode ({})",
        String::from_utf8_lossy(&xcode.stdout).trim()
    );

    // Stage 2: Mobile SDKs
    println!("\n── Stage 2: Mobile SDKs ──");
    let android_home = std::env::var("ANDROID_HOME").unwrap_or_else(|_| {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".android-sdk")
            .to_string_lossy()
            .into_owned()
    });

    // Java via mise
    let where_java = run_capture("mise", &["where", "java"], None)?;
    let java_home = if where_java.status.success() {
        let path = String::from_utf8_lossy(&where_java.stdout).trim().to_string();
        println!("  ✓ Java ({})", path);
        path
    } else {
        println!("  Installing Java {} via mise...", JAVA_VERSION);
        let status = Command::new("mise")
            .args(["use", "--global", &format!("java@{JAVA_VERSION}")])
            .status()?;
        if !status.success() {
            bail!("mise use java failed");
        }
        let path = capture_stdout("mise", &["where", "java"], None)?
            .trim()
            .to_string();
        println!("  ✓ Java installed");
        path
    };

    let sdkmanager = format!("{android_home}/cmdline-tools/latest/bin/sdkmanager");
    let avdmanager = format!("{android_home}/cmdline-tools/latest/bin/avdmanager");

    if Path::new(&sdkmanager).exists() {
        println!("  ✓ Android cmdline-tools ({})", android_home);
    } else {
        println!("  Installing Android cmdline-tools to {}...", android_home);
        std::fs::create_dir_all(&android_home)?;
        install_cmdline_tools(&android_home)?;
        println!("  ✓ Android cmdline-tools installed");
    }

    let env = sdk_env(&android_home, &java_home);

    // Accept licenses (non-fatal — yes-pipe with --licenses)
    println!("  Accepting Android SDK licenses...");
    let licenses_cmd = format!(
        "yes 2>/dev/null | '{sdkmanager}' --licenses --sdk_root='{android_home}' >/dev/null 2>&1 || true"
    );
    let _ = Command::new("sh")
        .args(["-c", &licenses_cmd])
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .status();

    install_sdk_pkg(&sdkmanager, &android_home, &env,
        format!("platforms;{PLATFORM_VERSION}"),
        format!("{android_home}/platforms/{PLATFORM_VERSION}"),
        &format!("Android platform {PLATFORM_VERSION}"))?;

    install_sdk_pkg(&sdkmanager, &android_home, &env,
        format!("build-tools;{BUILD_TOOLS_VERSION}"),
        format!("{android_home}/build-tools/{BUILD_TOOLS_VERSION}"),
        &format!("Android build-tools {BUILD_TOOLS_VERSION}"))?;

    install_sdk_pkg(&sdkmanager, &android_home, &env,
        "platform-tools".into(),
        format!("{android_home}/platform-tools"),
        "Android platform-tools (adb)")?;

    let ndk_marker = format!("{android_home}/ndk/{NDK_VERSION}/source.properties");
    if Path::new(&ndk_marker).exists() {
        println!("  ✓ Android NDK {}", NDK_VERSION);
    } else {
        let _ = std::fs::remove_dir_all(format!("{android_home}/ndk/{NDK_VERSION}"));
        println!("  Installing Android NDK {}...", NDK_VERSION);
        run_sdkmanager(&sdkmanager, &android_home, &env, &format!("ndk;{NDK_VERSION}"))?;
        println!("  ✓ Android NDK installed");
    }

    install_sdk_pkg(&sdkmanager, &android_home, &env,
        "emulator".into(),
        format!("{android_home}/emulator/emulator"),
        "Android emulator")?;

    let system_image = format!("system-images;{PLATFORM_VERSION};google_apis;arm64-v8a");
    let image_dir = format!("{android_home}/system-images/{PLATFORM_VERSION}/google_apis/arm64-v8a");
    if Path::new(&image_dir).exists() {
        println!("  ✓ System image {}", system_image);
    } else {
        println!("  Installing system image (ARM64)... this takes a while");
        run_sdkmanager(&sdkmanager, &android_home, &env, &system_image)?;
        println!("  ✓ System image installed");
    }

    // AVD
    let avd_list = capture_stdout(&avdmanager, &["list", "avd", "-c"], Some(&env)).unwrap_or_default();
    if avd_list.lines().any(|l| l.trim() == AVD_NAME) {
        println!("  ✓ AVD \"{}\"", AVD_NAME);
    } else {
        println!("  Creating AVD \"{}\"...", AVD_NAME);
        let status = Command::new(&avdmanager)
            .args([
                "create", "avd",
                "-n", AVD_NAME,
                "-k", &system_image,
                "--device", "pixel_6",
                "--force",
            ])
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .status()?;
        if !status.success() {
            bail!("avdmanager create avd failed");
        }
        println!("  ✓ AVD \"{}\" created", AVD_NAME);
    }

    // Stage 3: Rust Android targets
    println!("\n── Stage 3: Rust Android targets ──");
    let installed = capture_stdout("rustup", &["target", "list", "--installed"], None)
        .unwrap_or_default();
    for target in &[
        "aarch64-linux-android",
        "armv7-linux-androideabi",
        "i686-linux-android",
        "x86_64-linux-android",
    ] {
        if installed.lines().any(|l| l.trim() == *target) {
            println!("  ✓ {}", target);
        } else {
            println!("  Adding {}...", target);
            let status = Command::new("rustup")
                .args(["target", "add", target])
                .status()?;
            if !status.success() {
                bail!("rustup target add {} failed", target);
            }
            println!("  ✓ {} added", target);
        }
    }

    // Stage 4: iOS
    println!("\n── Stage 4: iOS deps ──");
    if cmd_exists("pod") {
        let ver = capture_first_word("pod", &["--version"]);
        println!("  ✓ CocoaPods {}", ver);
    } else {
        println!("  Installing CocoaPods...");
        let status = Command::new("gem").args(["install", "cocoapods"]).status()?;
        if !status.success() {
            bail!("gem install cocoapods failed");
        }
        println!("  ✓ CocoaPods installed");
    }

    println!("\n═══ Setup complete ═══");
    println!("\nEnvironment:");
    println!("  ANDROID_HOME={}", android_home);
    println!("  NDK_HOME={}/ndk/{}", android_home, NDK_VERSION);
    println!("  JAVA_HOME={}", java_home);
    println!("\nNext:");
    println!("  utm-dev mac dev          # macOS desktop dev mode");
    println!("  utm-dev ios sim          # iOS simulator");
    println!("  utm-dev android sim      # Android emulator");
    println!("  utm-dev windows build    # Windows .msi/.exe (VM auto-starts)");
    println!("  utm-dev linux build      # Linux .deb/.AppImage (VM auto-starts)");
    println!("  utm-dev doctor           # check everything");
    Ok(())
}

// ── Android SDK helpers ──────────────────────────────────────────────────────

fn sdk_env(android_home: &str, java_home: &str) -> Vec<(String, String)> {
    let path_extra = format!(
        "{android_home}/cmdline-tools/latest/bin:{android_home}/platform-tools:{android_home}/emulator"
    );
    let path = std::env::var("PATH")
        .map(|p| format!("{path_extra}:{p}"))
        .unwrap_or(path_extra);
    vec![
        ("ANDROID_HOME".into(), android_home.into()),
        ("JAVA_HOME".into(),    java_home.into()),
        ("PATH".into(),         path),
    ]
}

fn install_sdk_pkg(
    sdkmanager:   &str,
    android_home: &str,
    env:          &[(String, String)],
    package:      String,
    marker:       String,
    label:        &str,
) -> Result<()> {
    if Path::new(&marker).exists() {
        println!("  ✓ {}", label);
    } else {
        println!("  Installing {}...", label);
        run_sdkmanager(sdkmanager, android_home, env, &package)?;
        println!("  ✓ {} installed", label);
    }
    Ok(())
}

fn run_sdkmanager(
    sdkmanager:   &str,
    android_home: &str,
    env:          &[(String, String)],
    package:      &str,
) -> Result<()> {
    let status = Command::new(sdkmanager)
        .arg(format!("--sdk_root={android_home}"))
        .arg(package)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .status()
        .with_context(|| format!("running {sdkmanager}"))?;
    if !status.success() {
        bail!("{} {} failed", sdkmanager, package);
    }
    Ok(())
}

fn install_cmdline_tools(android_home: &str) -> Result<()> {
    let tmp_zip = std::env::temp_dir().join(format!(
        "cmdline-tools-{}.zip",
        std::process::id()
    ));

    let status = Command::new("curl")
        .args(["-sSfL", "-o"])
        .arg(&tmp_zip)
        .arg(CMDLINE_TOOLS_URL)
        .status()
        .context("curl cmdline-tools")?;
    if !status.success() {
        bail!("curl failed downloading cmdline-tools");
    }

    let tmp_extract = format!("{android_home}/cmdline-tools-tmp");
    let _ = std::fs::remove_dir_all(&tmp_extract);
    std::fs::create_dir_all(&tmp_extract)?;
    let status = Command::new("unzip")
        .args(["-qo"])
        .arg(&tmp_zip)
        .args(["-d"])
        .arg(&tmp_extract)
        .status()
        .context("unzip cmdline-tools")?;
    if !status.success() {
        bail!("unzip failed");
    }

    std::fs::create_dir_all(format!("{android_home}/cmdline-tools"))?;
    let _ = std::fs::remove_dir_all(format!("{android_home}/cmdline-tools/latest"));
    std::fs::rename(
        format!("{tmp_extract}/cmdline-tools"),
        format!("{android_home}/cmdline-tools/latest"),
    )?;
    let _ = std::fs::remove_dir(&tmp_extract);
    let _ = std::fs::remove_file(&tmp_zip);
    Ok(())
}

// ── Generic helpers ──────────────────────────────────────────────────────────

fn cmd_exists(cmd: &str) -> bool {
    which::which(cmd).is_ok()
}

fn capture_first_word(cmd: &str, args: &[&str]) -> String {
    capture_stdout(cmd, args, None)
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or("?")
        .to_string()
}

fn capture_stdout(cmd: &str, args: &[&str], env: Option<&[(String, String)]>) -> Result<String> {
    let mut c = Command::new(cmd);
    c.args(args);
    if let Some(e) = env {
        c.envs(e.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    let out = c.output()?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_capture(
    cmd:  &str,
    args: &[&str],
    env:  Option<&HashMap<String, String>>,
) -> Result<std::process::Output> {
    let mut c = Command::new(cmd);
    c.args(args);
    if let Some(e) = env {
        c.envs(e);
    }
    Ok(c.output()?)
}

fn sh(script: &str) -> Result<()> {
    let status = Command::new("sh").args(["-c", script]).status()?;
    if !status.success() {
        bail!("shell script failed: {}", script);
    }
    Ok(())
}

