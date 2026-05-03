# Persist mise's `cargo_binstall = true` setting in the user mise config.
# Belt-and-braces alongside the MISE_CARGO_BINSTALL env var — env vars are
# scoped to a process, settings file is permanent.
$cfg = "$env:USERPROFILE\AppData\Roaming\mise\config.toml"
$dir = Split-Path $cfg
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
if (-not (Test-Path $cfg)) {
    Set-Content $cfg "[settings]`ncargo_binstall = true`n" -Encoding UTF8
} elseif ((Get-Content $cfg -Raw) -notmatch 'cargo_binstall') {
    Add-Content $cfg "`n[settings]`ncargo_binstall = true`n" -Encoding UTF8
}
