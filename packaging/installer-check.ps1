# Checks a built installer on this PC: installs it silently, looks at what it put where,
# starts the program, then uninstalls and looks at what is left.
#
#   packaging\installer-check.ps1 -Setup <VaultKey-Setup-x.y.z.exe> -Version <x.y.z> [-DeleteData]
#
# It installs for the current user, like a person would, so run it on a clean machine: a
# GitHub runner or a virtual machine. -DeleteData also checks that the uninstaller can
# delete the app's data folder. That folder holds backups of saves on a real PC, so the
# switch is refused outside GitHub Actions.
param(
    [Parameter(Mandatory = $true)][string]$Setup,
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$DeleteData
)
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ($DeleteData -and $env:GITHUB_ACTIONS -ne "true") {
    throw "-DeleteData deletes the app's data folder and only runs in GitHub Actions."
}

$failures = New-Object System.Collections.Generic.List[string]
function Check([string]$what, [bool]$ok) {
    if ($ok) { Write-Output "PASS $what" } else { Write-Output "FAIL $what"; $failures.Add($what) }
}
function UninstallKey {
    Get-ChildItem "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall" -ErrorAction SilentlyContinue |
        Where-Object { $_.PSChildName -like "*3D860A2F-A8CD-433B-BF17-F4E790CDA03C*" }
}

$app = Join-Path $env:LOCALAPPDATA "Programs\Vault Key"
$exe = Join-Path $app "vault-key.exe"
$shortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Vault Key.lnk"
$data = Join-Path $env:LOCALAPPDATA "VaultKey"
$silent = @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART")

$install = Start-Process (Resolve-Path $Setup).Path -ArgumentList $silent -Wait -PassThru
Check "the installer finishes without an error (exit code $($install.ExitCode))" ($install.ExitCode -eq 0)
foreach ($file in @("vault-key.exe", "WebView2Loader.dll", "LICENSE.txt", "THIRD_PARTY_NOTICES.txt", "unins000.exe")) {
    Check "the program folder has $file" (Test-Path (Join-Path $app $file))
}
Check "the installed program is version $Version" ((Get-Item $exe).VersionInfo.ProductVersion -eq $Version)

Check "the Start menu has a Vault Key shortcut" (Test-Path $shortcut)
$shell = New-Object -ComObject Shell.Application
$item = $shell.Namespace((Split-Path $shortcut)).ParseName((Split-Path -Leaf $shortcut))
Check "the shortcut carries the app's taskbar ID" ($item.ExtendedProperty("System.AppUserModel.ID") -eq "VaultKey.App")
Check "the shortcut starts the installed program" ($item.ExtendedProperty("System.Link.TargetParsingPath") -eq $exe)

$key = UninstallKey
Check "Settings > Apps has an entry" ([bool]$key)
if ($key) {
    $entry = Get-ItemProperty $key.PSPath
    Check "the entry is named Vault Key, version $Version" ($entry.DisplayName -eq "Vault Key" -and $entry.DisplayVersion -eq $Version)
    Check "the entry names a publisher and shows the program's icon" ([bool]$entry.Publisher -and $entry.DisplayIcon -eq $exe)
}

# The program starts, stays up with its own window (not an error box), and a second start
# does not make a second copy.
$first = Start-Process $exe -PassThru
Start-Sleep -Seconds 10
$first.Refresh()
Check "the program starts and keeps running" (-not $first.HasExited)
Check "the program shows its window, titled Vault Key (title: '$($first.MainWindowTitle)')" ($first.MainWindowHandle -ne 0 -and $first.MainWindowTitle -eq "Vault Key")
$second = Start-Process $exe -PassThru
$ended = $second.WaitForExit(15000)
Check "a second start ends by itself (one copy at a time)" $ended
if (-not $ended) { Stop-Process -Id $second.Id -Force -ErrorAction SilentlyContinue }
Check "the program made its data folder" (Test-Path $data)

# The uninstaller refuses while the program runs, so stop it first, then wait for the
# window's WebView2 helpers: they hold files in the data folder for a moment after.
Stop-Process -Id $first.Id -Force -ErrorAction SilentlyContinue
Get-Process vault-key -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
$webView = Join-Path $data "WebView"
$deadline = (Get-Date).AddSeconds(30)
do {
    Start-Sleep -Seconds 1
    $helpers = @(Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" | Where-Object { $_.CommandLine -like "*$webView*" })
} while ($helpers.Count -gt 0 -and (Get-Date) -lt $deadline)
$helpers | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

$arguments = $silent
if ($DeleteData) { $arguments = $silent + "/DELETEDATA" }
$uninstall = Start-Process (Join-Path $app "unins000.exe") -ArgumentList $arguments -Wait -PassThru
Check "the uninstaller finishes without an error (exit code $($uninstall.ExitCode))" ($uninstall.ExitCode -eq 0)
# The uninstaller hands over to a copy of itself in the temp folder; give it a moment.
$deadline = (Get-Date).AddSeconds(30)
while ((Test-Path $app) -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 1 }
Check "the program folder is gone" (-not (Test-Path $app))
Check "the Start menu shortcut is gone" (-not (Test-Path $shortcut))
Check "the Settings > Apps entry is gone" (-not (UninstallKey))
if ($DeleteData) {
    Check "the data folder is gone when asked" (-not (Test-Path $data))
} else {
    Check "the data folder is kept" (Test-Path $data)
}

if ($failures.Count -gt 0) {
    throw "$($failures.Count) installer check(s) failed."
}
Write-Output "INSTALLER CHECK PASSED"
