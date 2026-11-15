# Checks that installing a new version over an older one keeps the user's data: installs
# the old setup and starts it once, puts settings and a backup in the data folder the way
# the old version keeps them, then installs the new setup over it and starts the new
# program, checking at each step that the data is still there, byte for byte.
#
#   packaging\upgrade-check.ps1 -OldSetup <VaultKey-Setup-a.b.c.exe> -NewSetup <VaultKey-Setup-x.y.z.exe> -Version <x.y.z>
#
# It installs for the current user and writes the app's data folder, so it only runs in
# GitHub Actions or Windows Sandbox, on a PC where Vault Key has no data folder yet (run
# installer-check.ps1 -DeleteData first, or nothing). It leaves the new version installed.
param(
    [Parameter(Mandatory = $true)][string]$OldSetup,
    [Parameter(Mandatory = $true)][string]$NewSetup,
    [Parameter(Mandatory = $true)][string]$Version
)
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# WDAGUtilityAccount is the only user inside Windows Sandbox, which is wiped when closed.
if ($env:GITHUB_ACTIONS -ne "true" -and $env:USERNAME -ne "WDAGUtilityAccount") {
    throw "The upgrade check writes the app's data folder and only runs in GitHub Actions or Windows Sandbox."
}

$failures = New-Object System.Collections.Generic.List[string]
function Check([string]$what, [bool]$ok) {
    if ($ok) { Write-Output "PASS $what" } else { Write-Output "FAIL $what"; $failures.Add($what) }
}

$app = Join-Path $env:LOCALAPPDATA "Programs\Vault Key"
$exe = Join-Path $app "vault-key.exe"
$data = Join-Path $env:LOCALAPPDATA "VaultKey"
$silent = @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART")
if (Test-Path $data) { throw "$data already exists; the upgrade check needs a PC without Vault Key's data." }
if (Test-Path $exe) { throw "Vault Key is already installed here; the upgrade check starts from the old version." }

# The game's save folder must exist for the app to start; an empty one will do.
$saveData = Join-Path ([Environment]::GetFolderPath("MyDocuments")) "My Games\Borderlands 2\WillowGame\SaveData"
New-Item -ItemType Directory -Force (Join-Path $saveData "76561190000000000") | Out-Null

function Hash([string]$path, [string]$algorithm = "SHA256") { (Get-FileHash $path -Algorithm $algorithm).Hash.ToLower() }

# An error box ("could not show its window") also has the title Vault Key, so the window
# only counts when its page is up: WebView2's helpers run on the app's own WebView folder.
# Only log lines after the first $logLinesBefore count, so an earlier run is not blamed.
function LogLines { $logFile = Join-Path $data "vault-key.log"; if (Test-Path $logFile) { @(Get-Content $logFile).Count } else { 0 } }
function CheckPageShown([string]$what, [int]$logLinesBefore = 0) {
    $webViewDir = Join-Path $data "WebView"
    $logFile = Join-Path $data "vault-key.log"
    $failed = @(if (Test-Path $logFile) { Get-Content $logFile | Select-Object -Skip $logLinesBefore | Where-Object { $_ -match "could not (show|open) its window|cannot use its data folder|panic:" } })
    # Windows Sandbox's own WebView2 Runtime cannot be loaded by apps, and Microsoft's installer
    # will not replace it, so there the page cannot be checked. GitHub Actions checks it.
    if ($env:USERNAME -eq "WDAGUtilityAccount" -and ($failed -match "Package dependency criteria could not be resolved")) {
        Write-Output "SKIP $what shows its page: Windows Sandbox's WebView2 Runtime can't be loaded by apps"
        return
    }
    $pages = @(Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" | Where-Object { $_.CommandLine -like "*$webViewDir*" })
    Check "$what shows its page (WebView2 is running for it)" ($pages.Count -gt 0)
    Check "$what logged no start-up failure" ($failed.Count -eq 0)
    $failed | Select-Object -Last 5 | ForEach-Object { Write-Output "  log: $_" }
}

function StartApp([string]$what) {
    $logBefore = LogLines
    $process = Start-Process $exe -PassThru
    Start-Sleep -Seconds 10
    $process.Refresh()
    Check "$what starts and keeps running" (-not $process.HasExited)
    Check "$what shows its window, titled Vault Key (title: '$($process.MainWindowTitle)')" ($process.MainWindowHandle -ne 0 -and $process.MainWindowTitle -eq "Vault Key")
    CheckPageShown $what $logBefore
}

# Stops the program and waits for the window's WebView2 helpers, which hold files in the
# data folder for a moment after.
function StopApp {
    Get-Process vault-key -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    $webView = Join-Path $data "WebView"
    $deadline = (Get-Date).AddSeconds(30)
    do {
        Start-Sleep -Seconds 1
        $helpers = @(Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" | Where-Object { $_.CommandLine -like "*$webView*" })
    } while ($helpers.Count -gt 0 -and (Get-Date) -lt $deadline)
    $helpers | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}

# 1. The old version, installed and started once, as a person would have.
$install = Start-Process (Resolve-Path $OldSetup).Path -ArgumentList $silent -Wait -PassThru
Check "the old setup installs (exit code $($install.ExitCode))" ($install.ExitCode -eq 0)
$oldExe = Hash $exe
$oldVersion = (Get-Item $exe).VersionInfo.ProductVersion
Write-Output "old version: $oldVersion"
StartApp "the old program"
StopApp
Check "the old program made its data folder" (Test-Path $data)
$webView = Join-Path $data "WebView"
$hadWebView = Test-Path $webView
Write-Output "after the old program: $((Get-ChildItem $data -Force | ForEach-Object Name) -join ', ')"

# 2. Settings and a backup as the old version writes them, an error log, and a line from
# some later version that this one does not know (it must be kept).
$slot = "76561190000000000/Save0003.sav"
$settings = @(
    "# Vault Key settings. One entry per line; lines that are not understood are kept.",
    "format 1",
    "save-folder $($saveData.ToLower().TrimEnd('\'))",
    "created $slot",
    "test-slot 76561190000000000/Save0004.sav character=0123456789abcdef",
    "theme purple",
    "from-a-later-version keep this line"
)
[System.IO.File]::WriteAllText((Join-Path $data "settings.txt"), (($settings -join "`n") + "`n"))

$backup = Join-Path $data "backups\76561190000000000\Save0003.sav\20261101-120000Z"
New-Item -ItemType Directory -Force $backup | Out-Null
$bytes = New-Object byte[] 40000
(New-Object System.Random 7).NextBytes($bytes)
$saveCopy = Join-Path $backup "Save0003.sav"
[System.IO.File]::WriteAllBytes($saveCopy, $bytes)
$info = @(
    "golden-key-backup 1",
    "slot $slot",
    "original-path $saveData\76561190000000000\Save0003.sav",
    "created-unix 1793534400",
    "created 2026-11-01 12:00:00 UTC",
    "reason Before saving changes",
    "size $($bytes.Length)",
    "sha1 $(Hash $saveCopy SHA1)",
    "name Upgrade Check"
)
[System.IO.File]::WriteAllText((Join-Path $backup "backup.txt"), (($info -join "`n") + "`n"))
# The saves the settings are about. They only need to be there: the records of a save that
# is gone are dropped on purpose, so a character the game later puts in that file starts
# locked. The app never writes them here (nothing is unlocked and nothing is saved).
$saves = @("Save0003.sav", "Save0004.sav") | ForEach-Object { Join-Path $saveData "76561190000000000\$_" }
$saves | ForEach-Object { [System.IO.File]::WriteAllBytes($_, $bytes) }
$saveHash = Hash $saves[0]
Add-Content (Join-Path $data "vault-key.log") "2026-11-01 12:00:00 a line from the old version"

$seeded = @(Get-ChildItem $data -Recurse -File | Where-Object { $_.FullName -notlike (Join-Path $data "WebView\*") } |
    ForEach-Object { $_.FullName.Substring($data.Length + 1) })
$before = @{}
foreach ($file in $seeded) { $before[$file] = Hash (Join-Path $data $file) }
Write-Output "data files: $($seeded -join ', ')"

# 3. The new setup over the old version: the program is replaced, the data is not touched.
$install = Start-Process (Resolve-Path $NewSetup).Path -ArgumentList $silent -Wait -PassThru
Check "the new setup installs over the old version (exit code $($install.ExitCode))" ($install.ExitCode -eq 0)
Check "the installed program is version $Version" ((Get-Item $exe).VersionInfo.ProductVersion -eq $Version)
Check "the program was replaced" ((Hash $exe) -ne $oldExe)
Check "it is installed in the same folder" (Test-Path (Join-Path $app "unins000.exe"))
Check "there is still one Settings > Apps entry" (@(Get-ChildItem "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall" |
    Where-Object { $_.PSChildName -like "*3D860A2F-A8CD-433B-BF17-F4E790CDA03C*" }).Count -eq 1)
foreach ($file in $seeded) {
    $path = Join-Path $data $file
    Check "the upgrade left $file as it was" ((Test-Path $path) -and (Hash $path) -eq $before[$file])
}
if ($hadWebView) { Check "the window's own data (WebView) is still there" (Test-Path $webView) }
Write-Output "after the upgrade: $((Get-ChildItem $data -Force | ForEach-Object Name) -join ', ')"

# 4. The new program on the old data.
StartApp "the new program"

# Setup does not replace a program that is running: it stops and changes nothing.
$newExe = Hash $exe
$again = Start-Process (Resolve-Path $NewSetup).Path -ArgumentList $silent -Wait -PassThru
Check "setup refuses while the program runs (exit code $($again.ExitCode))" ($again.ExitCode -ne 0)
Check "the running program's files were left alone" ((Hash $exe) -eq $newExe)
StopApp

# The new version may add to the settings (it ties records to characters, say), but every
# entry and the backup stay.
foreach ($file in $seeded | Where-Object { $_ -like "backups\*" }) {
    $path = Join-Path $data $file
    Check "the new program left $file as it was" ((Test-Path $path) -and (Hash $path) -eq $before[$file])
}
foreach ($save in $saves) {
    Check "the new program left $(Split-Path -Leaf $save) in the save folder as it was" ((Test-Path $save) -and (Hash $save) -eq $saveHash)
}
$now = @(Get-Content (Join-Path $data "settings.txt"))
foreach ($line in $settings | Select-Object -Skip 2) {
    Check "the settings still have '$line'" ([bool]($now | Where-Object { $_ -eq $line -or $_.StartsWith("$line character=") }))
}
$log = Get-Content (Join-Path $data "vault-key.log") -Raw
Check "the error log still has the old version's lines" ($log -like "*a line from the old version*")
Check "the new program started on the old settings without a problem" ($log -notlike "*could not start*")

if ($failures.Count -gt 0) {
    throw "$($failures.Count) upgrade check(s) failed."
}
Write-Output "UPGRADE CHECK PASSED"
