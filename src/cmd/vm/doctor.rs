//! `utm-dev vm doctor` — health checks inside a VM.
//!
//! Runs a profile-specific list of checks (mise on PATH, VS Build Tools
//! components on Windows, apt build-essential / libwebkit2gtk on Linux, …)
//! over SSH and reports pass/fail/known-blocked. Exits non-zero when any
//! actionable check fails — useful as a CI gate.

use crate::vm::{profiles, ssh};

pub fn run(name: &str) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    let session = ssh::connect(profile)
        .map_err(|e| anyhow::anyhow!("cannot SSH to '{name}': {e:#}\n  → utm-dev vm up --name {name}"))?;

    println!("══ utm-dev vm doctor — {} ══\n", name);

    let checks: Vec<(&str, &str)> = match profile.os {
        profiles::GuestOs::Linux => vec![
            ("mise on PATH",
             "command -v mise >/dev/null && mise --version || echo MISSING"),
            ("apt build-essential",
             "dpkg-query -W -f='${Status}' build-essential 2>/dev/null | grep -c 'ok installed' | grep -qx 1 && echo ok || echo MISSING"),
            ("apt libwebkit2gtk-4.1-dev (Tauri)",
             "dpkg-query -W -f='${Status}' libwebkit2gtk-4.1-dev 2>/dev/null | grep -c 'ok installed' | grep -qx 1 && echo ok || echo MISSING"),
            ("apt libwebkit2gtk-4.1-dev:amd64 (multiarch x86_64)",
             "dpkg-query -W -f='${Status}' libwebkit2gtk-4.1-dev:amd64 2>/dev/null | grep -c 'ok installed' | grep -qx 1 && echo ok || echo 'MISSING (run vm build --target x86-64 to install)'"),
            ("apt gcc-x86-64-linux-gnu (cross linker)",
             "command -v x86_64-linux-gnu-gcc >/dev/null && echo ok || echo MISSING"),
            ("xvfb-run (vm run)",
             "command -v xvfb-run >/dev/null && echo ok || echo MISSING"),
        ],
        profiles::GuestOs::Windows => vec![
            ("mise on PATH",
             "where mise 2>nul && mise --version || echo MISSING"),
            ("VS Build Tools install path",
             r#"if exist "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC" (echo ok) else (echo MISSING)"#),
            ("VS Hostarm64\\x64 cross-tools (link.exe)",
             r#"for /d %V in ("C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\*") do @if exist "%V\bin\Hostarm64\x64\link.exe" echo ok"#),
            ("VS Hostarm64\\arm64 native tools (BLOCKED)",
             r#"for /d %V in ("C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\*") do @if exist "%V\bin\Hostarm64\arm64\link.exe" (echo ok) else (echo BLOCKED_BY_MS)"#),
            ("WebView2 Runtime",
             r#"if exist "C:\Program Files (x86)\Microsoft\EdgeWebView" (echo ok) else (echo MISSING)"#),
            ("rustup default-host = x86_64",
             r#"powershell -NoProfile -Command "$r = (mise where rust 2>$null); if ($r) { & ($r + '\\rustup.exe') show 2>$null | Select-String 'Default host:.*x86_64' | ForEach-Object { 'ok' } } else { 'MISSING' }""#),
        ],
    };

    let mut real_failures = 0;
    let mut expected_failures = 0;
    for (label, cmd) in checks {
        let out = ssh::exec(&session, cmd).unwrap_or_else(|e| format!("ERR {e}"));
        let trimmed = out.trim();
        let blocked = trimmed.contains("BLOCKED_BY_MS");
        let pass = !trimmed.is_empty()
            && !trimmed.contains("MISSING")
            && !blocked
            && !trimmed.starts_with("ERR")
            && !trimmed.contains("could not find")
            && !trimmed.contains("not recognized");
        if pass {
            println!("  ✓ {label}");
        } else if blocked {
            println!("  ⚠ {label} (known-blocked, not actionable)");
            expected_failures += 1;
        } else {
            real_failures += 1;
            println!("  ✗ {label}");
            for line in trimmed.lines().take(3) {
                println!("      {line}");
            }
        }
    }

    println!();
    if real_failures == 0 {
        if expected_failures > 0 {
            println!("✓ all actionable checks passed ({expected_failures} known-blocked, see GAPS.md)");
        } else {
            println!("✓ all checks passed");
        }
    } else {
        println!("✗ {real_failures} check(s) failed");
        std::process::exit(1);
    }
    Ok(())
}
