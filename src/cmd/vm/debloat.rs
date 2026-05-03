//! `utm-dev vm debloat` — remove pre-installed Windows Store apps that have
//! no purpose on a build VM (Xbox, Bing News, Mail, Calendar, Solitaire, …).
//!
//! Removes both the per-user package AND the provisioned package so they
//! don't reinstall on next user creation. Idempotent — already-removed
//! apps are a silent no-op. Windows-only.

use crate::vm::{profiles, ssh};

/// Curated list of Microsoft Store / inbox apps that ship in retail Windows
/// images and have no place on a build VM. Each entry is a PREFIX matched
/// against `Get-AppxPackage -Name "<prefix>*"`, so `Microsoft.Xbox` removes
/// every Xbox-* package in one go.
///
/// Sourced by cross-referencing three canonical debloat projects:
///   - Raphire/Win11Debloat (Config/Apps.json, default-on entries)
///   - Sycnex/Windows10Debloater ($Bloatware array)
///   - ChrisTitusTech/winutil (config/tweaks.json WPFTweaksDeBloat.appx)
///
/// Every entry below is on at least 2 of the 3 lists.
///
/// Deliberately NOT in this list (would break things):
///   Microsoft.WindowsStore           — winget depends on it
///   Microsoft.DesktopAppInstaller    — winget itself
///   Microsoft.VCLibs.*               — runtime libraries
///   Microsoft.UI.Xaml.*              — UI framework runtime
///   Microsoft.WebView2.*             — we install/use it for Tauri
///   Microsoft.NET.*                  — .NET runtimes
///   Microsoft.ScreenSketch           — Snipping Tool, useful, on no canonical list
const DEBLOAT_PREFIXES: &[&str] = &[
    // News / weather / hubs
    "Microsoft.BingNews",
    "Microsoft.BingWeather",
    "Microsoft.News",
    "Microsoft.MicrosoftOfficeHub",
    // Help / getting-started
    "Microsoft.GetHelp",
    "Microsoft.Getstarted",
    "Microsoft.WindowsFeedbackHub",
    // Productivity-ish that a build VM doesn't need
    "Microsoft.Office.OneNote",
    "Microsoft.Office.Sway",
    "Microsoft.MicrosoftStickyNotes",
    "Microsoft.Todos",
    "Microsoft.WindowsAlarms",
    "Microsoft.WindowsMaps",
    "Microsoft.WindowsSoundRecorder",
    // Mail/Calendar/People/Messaging/Skype
    "Microsoft.WindowsCommunicationsApps", // Mail + Calendar
    "Microsoft.People",
    "Microsoft.Messaging",
    "Microsoft.SkypeApp",
    "Microsoft.YourPhone", // Phone Link
    // Solitaire / 3D / Mixed Reality
    "Microsoft.MicrosoftSolitaireCollection",
    "Microsoft.Microsoft3DViewer",
    "Microsoft.Print3D",
    "Microsoft.MixedReality.Portal",
    "Microsoft.NetworkSpeedTest",
    "Microsoft.OneConnect",
    // Xbox & gaming (prefix glob covers all sub-packages)
    "Microsoft.Xbox",
    "Microsoft.GamingApp",
    // Media
    "Microsoft.ZuneMusic", // Groove Music / Media Player
    "Microsoft.ZuneVideo", // Movies & TV
    "Clipchamp.Clipchamp",
    // Automate / DevHome (auto-installed, build VM has mise instead)
    "Microsoft.PowerAutomateDesktop",
    "Microsoft.Windows.DevHome",
    // Quick Assist
    "MicrosoftCorporationII.QuickAssist",
    // Consumer Teams (work Teams installed separately if needed)
    "MicrosoftTeams",
    "MSTeams",
];

pub fn run(name: &str, dry_run: bool) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    if profile.os != profiles::GuestOs::Windows {
        anyhow::bail!("vm debloat is Windows-only. Linux build VMs ship without Store-app bloat.");
    }
    ssh::check(profile)?;
    let session = ssh::connect(profile)?;

    let mode = if dry_run {
        "dry-run (no changes)"
    } else {
        "removing"
    };
    println!("→ vm debloat on {name} — {mode}");

    let (out, code) = ssh::exec_ps_windows(&session, &windows_debloat_script(dry_run))?;
    println!("{out}");
    if code != 0 {
        eprintln!("(some removals may have failed — non-fatal; re-running is safe)");
    }
    println!("✓ done");
    Ok(())
}

const SCRIPT_WINDOWS: &str = include_str!("../../../scripts/debloat/windows/main.ps1");
const SCRIPT_WINDOWS_ACTION: &str = include_str!("../../../scripts/debloat/windows/action.ps1");

fn windows_debloat_script(dry_run: bool) -> String {
    let prefixes_ps = DEBLOAT_PREFIXES
        .iter()
        .map(|p| format!("'{p}'"))
        .collect::<Vec<_>>()
        .join(",");
    SCRIPT_WINDOWS
        .replace("__PREFIXES__", &prefixes_ps)
        .replace(
            "__ACTION__",
            if dry_run { "" } else { SCRIPT_WINDOWS_ACTION },
        )
}
