# Runs inside Windows Sandbox, started by scripts/sandbox-check. The host folder with the
# setup program and these scripts is shared at C:\vk; results go to C:\vk\out.
#
# It runs the installer check twice (keeping, then deleting the app's data), then installs
# again and takes screenshots of the app and of Settings > Apps for a person to look at.
# The sandbox shuts itself down at the end, which throws everything inside it away.
param([Parameter(Mandatory = $true)][string]$Version)
$ErrorActionPreference = "Continue"
# Give the desktop a moment to finish starting.
Start-Sleep -Seconds 10
$out = "C:\vk\out"
New-Item -ItemType Directory -Force $out | Out-Null
Start-Transcript (Join-Path $out "log.txt") | Out-Null
$setup = "C:\vk\VaultKey-Setup-$Version.exe"
$result = "PASSED"

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
}
"@
function Screenshot([string]$name) {
    $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
    $bitmap.Save((Join-Path $out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose(); $bitmap.Dispose()
}

foreach ($deleteData in @($false, $true)) {
    Write-Output "== installer check, delete data: $deleteData"
    try {
        if ($deleteData) { & C:\vk\installer-check.ps1 -Setup $setup -Version $Version -DeleteData }
        else { & C:\vk\installer-check.ps1 -Setup $setup -Version $Version }
    }
    catch { Write-Output "ERROR $_"; $result = "FAILED" }
}

Write-Output "== screenshots"
Start-Process $setup -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait
$exe = Join-Path $env:LOCALAPPDATA "Programs\Vault Key\vault-key.exe"
# Started from the Start menu shortcut, the way a person starts it.
Start-Process (Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Vault Key.lnk")
Start-Sleep -Seconds 12
$app = Get-Process vault-key -ErrorAction SilentlyContinue | Where-Object MainWindowHandle -ne 0 | Select-Object -First 1
if ($app) {
    [Win]::ShowWindow($app.MainWindowHandle, 3) | Out-Null   # maximized
    [Win]::SetForegroundWindow($app.MainWindowHandle) | Out-Null
    Start-Sleep -Seconds 2
    Screenshot "app"
    [Win]::ShowWindow($app.MainWindowHandle, 9) | Out-Null   # restored, its own size
    Start-Sleep -Seconds 2
    Screenshot "app-window"
    # A second start brings the first window back instead of opening another.
    [Win]::ShowWindow($app.MainWindowHandle, 6) | Out-Null   # minimized
    Start-Sleep -Seconds 1
    Start-Process $exe
    Start-Sleep -Seconds 4
    Screenshot "second-start"
}
else { Write-Output "FAIL the app shows no window"; $result = "FAILED" }
Get-Process vault-key -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Process "ms-settings:appsfeatures"
Start-Sleep -Seconds 8
Screenshot "settings-apps"

Write-Output "SANDBOX CHECK $result"
Stop-Transcript | Out-Null
Set-Content (Join-Path $out "done.txt") $result
shutdown.exe /s /t 2
