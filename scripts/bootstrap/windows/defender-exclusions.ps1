# Add Windows Defender exclusions for cargo/mise/build paths.
#
# Defender's real-time scan briefly locks freshly-written files during cargo
# extract/compile. The lock manifests as "The process cannot access the file
# because it is being used by another process" partway through `mise install`
# or `cargo build`. Adding exclusions for the build dirs eliminates this —
# standard practice across the Rust/JetBrains/Microsoft dev-VM ecosystem.
#
# Idempotent: Add-MpPreference no-ops if the path is already excluded.
$paths = @(
  'D:\target',
  "$env:USERPROFILE\.cargo",
  "$env:USERPROFILE\.rustup",
  "$env:USERPROFILE\.local\share\mise",
  "$env:USERPROFILE\AppData\Local\mise",
  "$env:USERPROFILE\.utm-dev-build"
)
foreach ($p in $paths) {
  try { Add-MpPreference -ExclusionPath $p -ErrorAction Stop } catch { }
}
