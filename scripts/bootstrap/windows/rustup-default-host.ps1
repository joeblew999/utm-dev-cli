# Switch rustup's default-host to x86_64.
#
# VS Build Tools on ARM64 Windows hosts ships only Hostarm64\x64 and Hostarm64\x86
# cross-tools — there's no Hostarm64\arm64 native toolchain at the time of writing.
# A project's `mise.toml` declaring `rust = "stable"` would otherwise install the
# host-default `aarch64-pc-windows-msvc` toolchain, which fails to link anything
# (no ARM64 link.exe). Forcing rustup's default-host to x86_64 here means the
# toolchain mise installs is x86_64, which links cleanly with Hostarm64\x64\link.exe
# and runs under Windows ARM64's native x64 emulation.
#
# This is the one place we touch a runtime tool's config, and only because the
# alternative is "Windows builds don't work at all on this VM until each user
# discovers and works around it themselves."
$candidates = @(
  'D:\mise\installs\rust\stable\rustup.exe',
  "$env:USERPROFILE\.local\share\mise\installs\rust\stable\rustup.exe"
)
$rustup = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($rustup) {
  & $rustup set default-host x86_64-pc-windows-msvc
  & $rustup default --force-non-host stable-x86_64-pc-windows-msvc
}
