//! `utm-dev vm debloat` — remove pre-installed Windows Store apps that have
//! no purpose on a build VM (Xbox, Bing News, Mail, Calendar, Solitaire, …).
//!
//! Removes both the per-user package AND the provisioned package so they
//! don't reinstall on next user creation. Idempotent — already-removed
//! apps are a silent no-op. Windows-only.

use base64::Engine;

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

    let script = windows_debloat_script(dry_run);
    let utf16: Vec<u8> = script
        .encode_utf16()
        .flat_map(|c| c.to_le_bytes())
        .collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&utf16);
    let cmd = format!("powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand {encoded}");

    let (out, code) = ssh::exec_with_exit(&session, &cmd)?;
    println!("{out}");
    if code != 0 {
        eprintln!("(some removals may have failed — non-fatal; re-running is safe)");
    }
    println!("✓ done");
    Ok(())
}

fn windows_debloat_script(dry_run: bool) -> String {
    let prefixes_ps = DEBLOAT_PREFIXES
        .iter()
        .map(|p| format!("'{p}'"))
        .collect::<Vec<_>>()
        .join(",");

    let action = if dry_run {
        ""
    } else {
        r#"
foreach ($p in $found) {
  Write-Host ('  removing  {0}...' -f $p.Name) -NoNewline
  try {
    # Per-user package
    Get-AppxPackage -Name $p.Name -AllUsers -ErrorAction SilentlyContinue |
      Remove-AppxPackage -AllUsers -ErrorAction SilentlyContinue
    # Provisioned (so new users don't get it back)
    Get-AppxProvisionedPackage -Online -ErrorAction SilentlyContinue |
      Where-Object { $_.DisplayName -eq $p.Name } |
      ForEach-Object {
        Remove-AppxProvisionedPackage -Online -PackageName $_.PackageName -ErrorAction SilentlyContinue | Out-Null
      }
    Write-Host ' ok'
  } catch {
    Write-Host (' FAILED: {0}' -f $_.Exception.Message)
  }
}
"#
    };

    format!(
        r#"$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding           = [System.Text.Encoding]::UTF8

$prefixes = @({prefixes_ps})

$drive   = Get-PSDrive C
$before  = $drive.Free
Write-Host ('C: free before: {{0:N1}} GB' -f ($before/1GB))
Write-Host ''
Write-Host '--- Scanning installed Appx packages ---'

$found = @()
foreach ($prefix in $prefixes) {{
  Get-AppxPackage -AllUsers -Name "$prefix*" -ErrorAction SilentlyContinue | ForEach-Object {{
    # Skip system-pinned packages — they live under C:\Windows\SystemApps\
    # and Windows refuses Remove-AppxPackage on them (error 0x80070032).
    if ($_.SignatureKind -eq 'System') {{ return }}
    if ($_.InstallLocation -and $_.InstallLocation -like 'C:\Windows\SystemApps\*') {{ return }}
    if ($found.Name -notcontains $_.Name) {{
      $found += $_
    }}
  }}
}}

if ($found.Count -eq 0) {{
  Write-Host '  (no debloat-list packages installed — already clean)'
  return
}}

foreach ($p in $found) {{
  Write-Host ('  found     {{0}}' -f $p.Name)
}}
Write-Host ''
Write-Host ('Found {{0}} packages to remove.' -f $found.Count)
{action}

$drive2 = Get-PSDrive C
$after  = $drive2.Free
$freed  = $after - $before
function Fmt($b) {{
  if (-not $b -or $b -le 0) {{ return '0 B' }}
  if ($b -ge 1GB) {{ return ('{{0:N1}} GB' -f ($b/1GB)) }}
  if ($b -ge 1MB) {{ return ('{{0:N0}} MB' -f ($b/1MB)) }}
  return ('{{0:N0}} KB' -f ($b/1KB))
}}
Write-Host ''
Write-Host ('Freed:   {{0}}' -f (Fmt $freed))
Write-Host ('C: free: {{0:N1}} GB -> {{1:N1}} GB' -f ($before/1GB), ($after/1GB))
"#,
    )
}
