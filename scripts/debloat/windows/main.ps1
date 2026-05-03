# Remove pre-installed Windows Store apps that have no purpose on a build VM.
# Scans, then (unless dry-run) removes both the per-user package AND the
# provisioned package so they don't reinstall on next user creation.
# The Rust caller substitutes __PREFIXES__ (comma-separated PS string list)
# and __ACTION__ (the actual remove pass — empty in dry-run mode).

$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding           = [System.Text.Encoding]::UTF8

$prefixes = @(__PREFIXES__)

$drive   = Get-PSDrive C
$before  = $drive.Free
Write-Host ('C: free before: {0:N1} GB' -f ($before/1GB))
Write-Host ''
Write-Host '--- Scanning installed Appx packages ---'

$found = @()
foreach ($prefix in $prefixes) {
  Get-AppxPackage -AllUsers -Name "$prefix*" -ErrorAction SilentlyContinue | ForEach-Object {
    # Skip system-pinned packages — they live under C:\Windows\SystemApps\
    # and Windows refuses Remove-AppxPackage on them (error 0x80070032).
    if ($_.SignatureKind -eq 'System') { return }
    if ($_.InstallLocation -and $_.InstallLocation -like 'C:\Windows\SystemApps\*') { return }
    if ($found.Name -notcontains $_.Name) {
      $found += $_
    }
  }
}

if ($found.Count -eq 0) {
  Write-Host '  (no debloat-list packages installed — already clean)'
  return
}

foreach ($p in $found) {
  Write-Host ('  found     {0}' -f $p.Name)
}
Write-Host ''
Write-Host ('Found {0} packages to remove.' -f $found.Count)
__ACTION__

$drive2 = Get-PSDrive C
$after  = $drive2.Free
$freed  = $after - $before
function Fmt($b) {
  if (-not $b -or $b -le 0) { return '0 B' }
  if ($b -ge 1GB) { return ('{0:N1} GB' -f ($b/1GB)) }
  if ($b -ge 1MB) { return ('{0:N0} MB' -f ($b/1MB)) }
  return ('{0:N0} KB' -f ($b/1KB))
}
Write-Host ''
Write-Host ('Freed:   {0}' -f (Fmt $freed))
Write-Host ('C: free: {0:N1} GB -> {1:N1} GB' -f ($before/1GB), ($after/1GB))
