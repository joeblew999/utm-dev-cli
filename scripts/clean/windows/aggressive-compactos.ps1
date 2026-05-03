# Compress system files in-place (CompactOS). Slow but frees ~2 GB on
# Windows 11. Idempotent — exits early if already in Compact state.
$state = (& compact.exe /CompactOS:query | Out-String)
if ($state -match 'system is in the Compact state') {
  '  already in Compact state'
} else {
  & compact.exe /CompactOS:always | Out-Null
  '  compacted'
}
