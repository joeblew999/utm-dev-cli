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
