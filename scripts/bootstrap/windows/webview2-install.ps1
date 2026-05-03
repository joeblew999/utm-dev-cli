# Install WebView2 Runtime via the Evergreen Bootstrapper (~150 KB).
# winget install --id Microsoft.EdgeWebView2Runtime fails on fresh Vagrant
# Windows boxes ("No installed package found...") because winget's Store
# source isn't always primed. The Evergreen Bootstrapper at
# https://go.microsoft.com/fwlink/p/?LinkId=2124703 is the supported
# headless install path Microsoft documents.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Invoke-WebRequest -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile 'C:\webview2_setup.exe' -UseBasicParsing
Start-Process 'C:\webview2_setup.exe' -ArgumentList '/silent','/install' -Wait -NoNewWindow
