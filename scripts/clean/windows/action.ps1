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
