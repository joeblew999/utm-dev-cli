//! `utm-dev vm clean` — categorized disk cleanup inside a guest VM.
//!
//! Three modes, controlled by flags on the `Clean` subcommand:
//!   default       — transient caches (idempotent, safe)
//!   `--deep`      — also nuke cargo target/registry + mise installs
//!   `--aggressive` — also one-shot Windows tweaks (hibernation off,
//!                    CompactOS, VSS clear, pagefile to D:, event logs).
//!                    Frees the most space; some require reboot to apply.
//!
//! Phase 1 (transient clean) runs as one PowerShell script over SSH.
//! Phase 2 (aggressive tweaks) runs as separate per-step SSH execs so each
//! step's outcome is visible even if a later step encounters an issue.
//! Windows PS goes through [`ssh::exec_ps_windows`] which encodes the script
//! and strips PowerShell's CLIXML noise from the output.

use crate::vm::{profiles, ssh};

pub fn run(name: &str, deep: bool, aggressive: bool, dry_run: bool) -> anyhow::Result<()> {
    let profile = profiles::get(name)?;
    ssh::check(profile)?;
    let session = ssh::connect(profile)?;

    if aggressive && profile.os != profiles::GuestOs::Windows {
        anyhow::bail!("--aggressive is Windows-only (Linux build VMs don't have these knobs)");
    }

    let mode = match (deep, aggressive, dry_run) {
        (_, _, true) => "dry-run (no changes)",
        (_, true, false) => "aggressive (one-shot Windows tweaks)",
        (true, false, false) => "deep (incl. cargo + mise caches)",
        _ => "default (keeps build caches)",
    };
    println!("→ vm clean on {name} — {mode}");

    match profile.os {
        profiles::GuestOs::Linux => {
            let (out, code) = ssh::exec_with_exit(&session, &linux_clean_script(deep, dry_run))?;
            println!("{out}");
            if code != 0 {
                eprintln!("(some cleanup steps may have failed — non-fatal)");
            }
        }
        profiles::GuestOs::Windows => {
            let (out, code) = ssh::exec_ps_windows(&session, &windows_clean_script(deep, dry_run))?;
            println!("{out}");
            if code != 0 {
                eprintln!("(some transient-clean steps may have failed — non-fatal)");
            }
        }
    }

    if aggressive && profile.os == profiles::GuestOs::Windows {
        run_aggressive_tweaks(&session, dry_run)?;
    }

    println!("✓ done");
    Ok(())
}

/// One-shot Windows tweaks, each as a separate SSH exec. Idempotent.
/// Per-step rather than one big script so SSH stream drops don't hide
/// later steps' output, and each step's outcome is visible.
fn run_aggressive_tweaks(session: &ssh::Session, dry_run: bool) -> anyhow::Result<()> {
    let steps: &[(&str, &str)] = &[
        (
            "Hibernation: powercfg /h off",
            r#"if (Test-Path "$env:SystemRoot\hiberfil.sys") {
  $sz = (Get-Item -LiteralPath "$env:SystemRoot\hiberfil.sys" -Force -ErrorAction SilentlyContinue).Length
  & powercfg.exe /h off | Out-Null
  if ($sz -ge 1GB) { ('  freed ~{0:N1} GB' -f ($sz/1GB)) } else { ('  freed ~{0:N0} MB' -f ($sz/1MB)) }
} else { '  hiberfil.sys absent (already off)' }"#,
        ),
        (
            "VSS shadows: vssadmin delete shadows /all",
            r#"$o = (& vssadmin.exe list shadows /for=C: 2>&1 | Out-String)
if ($o -match 'No items found' -or $o -match 'could not be found') {
  '  no shadows present'
} else {
  & vssadmin.exe delete shadows /for=C: /all /quiet | Out-Null
  '  shadows cleared'
}"#,
        ),
        (
            "CompactOS: compress system files (slow)",
            r#"$state = (& compact.exe /CompactOS:query | Out-String)
if ($state -match 'system is in the Compact state') {
  '  already in Compact state'
} else {
  & compact.exe /CompactOS:always | Out-Null
  '  compacted'
}"#,
        ),
        (
            "Pagefile: move to D:\\pagefile.sys (reboot to apply)",
            r#"if (-not (Test-Path 'D:\')) { '  D: not present, skip'; return }
try {
  $cs = Get-CimInstance -ClassName Win32_ComputerSystem -ErrorAction Stop
  if (-not $cs.AutomaticManagedPagefile) { '  already custom-managed (skip)'; return }
  Set-CimInstance -InputObject $cs -Property @{ AutomaticManagedPagefile = $false } -ErrorAction Stop
  Get-CimInstance -ClassName Win32_PageFileSetting -ErrorAction SilentlyContinue |
    Remove-CimInstance -ErrorAction SilentlyContinue
  New-CimInstance -ClassName Win32_PageFileSetting -Property @{
    Name = 'D:\pagefile.sys'; InitialSize = 0; MaximumSize = 0
  } -ErrorAction Stop | Out-Null
  '  set to D: (reboot required)'
} catch { '  skipped: ' + $_.Exception.Message }"#,
        ),
        (
            "Event logs: wevtutil cl (skipping SSH/Security/System/Setup)",
            r#"$skip = @('OpenSSH','-Security','^Security$','^System$','^Setup$')
$n = 0
foreach ($log in (& wevtutil.exe el 2>$null)) {
  $s = $false
  foreach ($pat in $skip) { if ($log -match $pat) { $s = $true; break } }
  if (-not $s) { & wevtutil.exe cl "$log" 2>$null; $n++ }
}
('  cleared {0} channels' -f $n)"#,
        ),
    ];

    println!();
    println!("--- Aggressive (one-shot Windows tweaks) ---");
    let before = ps_c_free_gb(session).unwrap_or(0.0);

    for (label, ps) in steps {
        if dry_run {
            println!("  [dry-run] {label}");
            continue;
        }
        print!("  {label}...");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        match ssh::exec_ps_windows(session, ps) {
            Ok((out, _code)) => {
                println!();
                for line in out.lines() {
                    if !line.trim().is_empty() {
                        println!("{line}");
                    }
                }
            }
            Err(e) => println!(" SSH error: {e}"),
        }
    }

    if !dry_run {
        let after = ps_c_free_gb(session).unwrap_or(0.0);
        println!();
        println!(
            "C: free: {before:.1} GB -> {after:.1} GB ({:+.1} GB)",
            after - before
        );
    }
    Ok(())
}

fn ps_c_free_gb(session: &ssh::Session) -> Option<f64> {
    let (out, code) = ssh::exec_ps_windows(session, "(Get-PSDrive C).Free / 1GB").ok()?;
    if code != 0 {
        return None;
    }
    out.trim().parse::<f64>().ok()
}

/// PowerShell that scans known cleanup categories, prints sizes, then
/// removes each (unless dry_run). Categories are deliberately narrow —
/// each one names a specific disk hog we've actually seen on a build VM.
/// C: drive only. D: (cargo target / mise / sccache) is hands-off in
/// default mode; deep mode adds it.
fn windows_clean_script(deep: bool, dry_run: bool) -> String {
    let mut blocks = String::from(
        r#"$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding           = [System.Text.Encoding]::UTF8
$targets = @(
  @{ N='utm-dev build/run logs';     P=@("$env:USERPROFILE\.utm-dev-build", "$env:USERPROFILE\.utm-dev-run") },
  @{ N='Bootstrap installers (C:\)'; P=@('C:\vs_buildtools.exe','C:\webview2_setup.exe','C:\bootstrap-step.ps1','C:\bootstrap-step-done.txt','C:\vs-exit.txt') },
  @{ N='User temp';                  P=@("$env:USERPROFILE\AppData\Local\Temp") },
  @{ N='System temp';                P=@('C:\Windows\Temp') },
  @{ N='Windows Update cache';       P=@('C:\Windows\SoftwareDistribution\Download') },
  @{ N='VS Package Cache leftovers'; P=@('C:\ProgramData\Package Cache') },
  @{ N='Crash dumps';                P=@("$env:USERPROFILE\AppData\Local\CrashDumps", 'C:\Windows\Memory.dmp', 'C:\Windows\Minidump') },
  @{ N='Recycle Bin';                P=@('C:\$Recycle.Bin') }
"#,
    );
    if deep {
        blocks.push_str(
            r#"  ,
  @{ N='[deep] cargo target on D:\';   P=@('D:\target') },
  @{ N='[deep] cargo registry cache';  P=@("$env:USERPROFILE\.cargo\registry\cache","$env:USERPROFILE\.cargo\registry\src") },
  @{ N='[deep] sccache';               P=@("$env:USERPROFILE\AppData\Local\Mozilla\sccache") },
  @{ N='[deep] mise tool installs';    P=@("$env:USERPROFILE\.local\share\mise\installs") }
"#,
        );
    }
    blocks.push_str(")\n");

    let action = if dry_run {
        ""
    } else {
        r#"
foreach ($p in $plan) {
  Write-Host ('  {0,-40} cleaning... ' -f $p.N) -NoNewline
  foreach ($path in $p.P) {
    if (Test-Path -LiteralPath $path) {
      Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue
    }
  }
  Write-Host 'done'
}

Write-Host ''
Write-Host '--- DISM /StartComponentCleanup /ResetBase ---'
& dism.exe /Online /Cleanup-Image /StartComponentCleanup /ResetBase | Out-Null
Write-Host '  (DISM finished)'
"#
    };

    blocks.push_str(&format!(
        r#"
function Bytes($paths) {{
  $sum = [int64]0
  foreach ($p in $paths) {{
    if (Test-Path -LiteralPath $p) {{
      $m = Get-ChildItem -LiteralPath $p -Recurse -Force -ErrorAction SilentlyContinue |
           Measure-Object -Property Length -Sum
      if ($m -and $m.Sum) {{ $sum += [int64]$m.Sum }}
    }}
  }}
  return $sum
}}
function Fmt($b) {{
  if (-not $b -or $b -le 0) {{ return '0 B' }}
  if ($b -ge 1GB) {{ return ('{{0:N1}} GB' -f ($b/1GB)) }}
  if ($b -ge 1MB) {{ return ('{{0:N0}} MB' -f ($b/1MB)) }}
  if ($b -ge 1KB) {{ return ('{{0:N0}} KB' -f ($b/1KB)) }}
  return ('{{0}} B' -f $b)
}}

$drive  = Get-PSDrive C
$before = $drive.Free
$beforeUsed = $drive.Used
Write-Host ('C: free before: {{0:N1}} GB / {{1:N1}} GB total' -f ($before/1GB), (($before+$beforeUsed)/1GB))
Write-Host ''
Write-Host '--- Scanning ---'

$plan = @()
foreach ($t in $targets) {{
  $b = Bytes $t.P
  Write-Host ('  {{0,-40}} {{1}}' -f $t.N, (Fmt $b))
  if ($b -gt 0) {{ $plan += @{{ N=$t.N; P=$t.P; B=$b }} }}
}}
$total = ($plan | ForEach-Object {{ $_.B }} | Measure-Object -Sum).Sum
Write-Host ('  {{0,-40}} {{1}}' -f '— scan total —', (Fmt $total))
Write-Host ''

if ($plan.Count -eq 0) {{
  Write-Host '(no transient targets to clean)'
}}
{action}

$drive2 = Get-PSDrive C
$after  = $drive2.Free
$freed  = $after - $before
Write-Host ''
Write-Host ('Freed:    {{0}}' -f (Fmt $freed))
Write-Host ('C: free:  {{0:N1}} GB -> {{1:N1}} GB' -f ($before/1GB), ($after/1GB))
"#
    ));

    blocks
}

/// Linux equivalent — categorized scan + clean, du-driven sizes.
fn linux_clean_script(deep: bool, dry_run: bool) -> String {
    let deep_block = if deep {
        r#"
  '[deep] cargo target on /opt/build|/opt/build/target'
  '[deep] cargo registry|'$HOME'/.cargo/registry/cache:'$HOME'/.cargo/registry/src'
  '[deep] sccache|'$HOME'/.cache/sccache'
  '[deep] mise tool installs|'$HOME'/.local/share/mise/installs'"#
    } else {
        ""
    };

    let action = if dry_run {
        ""
    } else {
        r#"
echo
echo "--- Cleaning ---"
for entry in "${plan[@]}"; do
  label="${entry%%|*}"; rest="${entry#*|}"; rest="${rest#*|}"
  printf '  %-40s cleaning... ' "$label"
  IFS=':' read -r -a paths <<< "$rest"
  for p in "${paths[@]}"; do
    [ -e "$p" ] && rm -rf "$p" 2>/dev/null
  done
  echo done
done

if command -v apt-get >/dev/null 2>&1; then
  echo
  echo "--- apt clean / autoremove ---"
  sudo apt-get -y clean   >/dev/null 2>&1 || true
  sudo apt-get -y autoclean >/dev/null 2>&1 || true
fi

if command -v journalctl >/dev/null 2>&1; then
  sudo journalctl --vacuum-time=2d >/dev/null 2>&1 || true
fi
"#
    };

    format!(
        r#"set +e
fmt() {{ b=$1; if [ "$b" -ge 1073741824 ]; then awk -v b="$b" 'BEGIN{{printf "%.1f GB",b/1073741824}}'; elif [ "$b" -ge 1048576 ]; then awk -v b="$b" 'BEGIN{{printf "%d MB",b/1048576}}'; elif [ "$b" -ge 1024 ]; then awk -v b="$b" 'BEGIN{{printf "%d KB",b/1024}}'; else echo "$b B"; fi; }}
bytes() {{
  total=0
  IFS=':' read -r -a paths <<< "$1"
  for p in "${{paths[@]}}"; do
    [ -e "$p" ] || continue
    sz=$(du -sb "$p" 2>/dev/null | awk '{{print $1}}')
    [ -n "$sz" ] && total=$((total + sz))
  done
  echo "$total"
}}

# label|path1[:path2[:...]]
targets=(
  'utm-dev build/run logs|'$HOME'/.utm-dev-build:'$HOME'/.utm-dev-run:/tmp/utm-dev-*'
  'apt cache|/var/cache/apt/archives'
  'journal logs|/var/log/journal'
  'old crash reports|/var/crash'
  'temp files (>2 days old)|/tmp'{deep_block}
)

before_kb=$(df --output=avail / | tail -n 1 | tr -d ' ')
before=$((before_kb * 1024))
echo "/ free before: $(fmt $before)"
echo
echo "--- Scanning ---"
plan=()
for t in "${{targets[@]}}"; do
  label="${{t%%|*}}"; paths="${{t#*|}}"
  b=$(bytes "$paths")
  printf '  %-40s %s\n' "$label" "$(fmt $b)"
  [ "$b" -gt 0 ] && plan+=("$label|0|$paths")
done
{action}

after_kb=$(df --output=avail / | tail -n 1 | tr -d ' ')
after=$((after_kb * 1024))
freed=$((after - before))
echo
echo "Freed:   $(fmt $freed)"
echo "/ free:  $(fmt $before) -> $(fmt $after)"
"#
    )
}
