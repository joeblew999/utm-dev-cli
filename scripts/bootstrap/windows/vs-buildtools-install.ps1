# Install VS Build Tools with the C++ workload + ARM64 cross-tools.
# Note: requesting Microsoft.VisualStudio.Component.VC.Tools.ARM64 is best-effort
# — on ARM64 hosts the installer accepts the flag but doesn't actually place
# Hostarm64\arm64 native tools (BLOCKED_BY_MS, see GAPS.md). What WE rely on is
# Hostarm64\x64\link.exe, which IS installed by VCTools + --includeRecommended.
$p = Start-Process -FilePath 'C:\vs_buildtools.exe' -ArgumentList @(
    '--add', 'Microsoft.VisualStudio.Workload.VCTools',
    '--add', 'Microsoft.VisualStudio.Component.VC.Tools.ARM64',
    '--add', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
    '--add', 'Microsoft.VisualStudio.Component.Windows11SDK.22621',
    '--includeRecommended', '--quiet', '--norestart', '--wait'
) -Wait -NoNewWindow -PassThru
$p.ExitCode | Out-File 'C:\vs-exit.txt'
