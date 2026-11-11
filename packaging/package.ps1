# Builds what a release hands out, from an already built program:
#   VaultKey-Setup-<version>.exe       the installer
#   VaultKey-<version>-portable.zip    the program folder, for people who do not want to install
#   SHA256SUMS.txt                      checksums of both
#
#   packaging\package.ps1 -ExeDir <folder with vault-key.exe and WebView2Loader.dll> -OutDir <folder>
#
# The version comes from the workspace version in Cargo.toml: the one place it is written.
# GitHub Actions runs this for a release; scripts/package runs it from WSL.
param(
    [Parameter(Mandatory = $true)][string]$ExeDir,
    [Parameter(Mandatory = $true)][string]$OutDir,
    # ISCC.exe of Inno Setup 7. Looked for on PATH when not given.
    [string]$Iscc = ""
)
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
$ProgressPreference = "SilentlyContinue"

$repo = Split-Path -Parent $PSScriptRoot
$versionLine = Get-Content (Join-Path $repo "Cargo.toml") | Where-Object { $_ -match '^version\s*=\s*"([^"]+)"' } | Select-Object -First 1
if (-not $versionLine) { throw "No version found in Cargo.toml." }
$version = [regex]::Match($versionLine, '"([^"]+)"').Groups[1].Value

if (-not $Iscc) {
    $found = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if (-not $found) { throw "ISCC.exe (Inno Setup) was not found. Pass -Iscc <path>." }
    $Iscc = $found.Source
}

$exe = Join-Path $ExeDir "vault-key.exe"
$loader = Join-Path $ExeDir "WebView2Loader.dll"
foreach ($file in @($exe, $loader)) {
    if (-not (Test-Path $file)) { throw "Missing $file. Build the release program first." }
}
$built = (Get-Item $exe).VersionInfo.ProductVersion
if ($built -ne $version) { throw "vault-key.exe says version $built but Cargo.toml says $version. Rebuild the program." }

New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("gk-package-" + [System.Guid]::NewGuid().ToString("N"))
$portableName = "VaultKey-$version-portable"
$stage = Join-Path $work $portableName
New-Item -ItemType Directory -Force $stage | Out-Null
try {
    Copy-Item $exe, $loader $stage
    Copy-Item (Join-Path $repo "LICENSE") (Join-Path $stage "LICENSE.txt")
    Copy-Item (Join-Path $repo "THIRD_PARTY_NOTICES.md") (Join-Path $stage "THIRD_PARTY_NOTICES.txt")

    # The portable zip is the program folder as it is.
    $zip = Join-Path $OutDir "$portableName.zip"
    Remove-Item $zip -ErrorAction SilentlyContinue
    # Written entry by entry so the paths inside use forward slashes whichever PowerShell runs
    # this (Windows PowerShell 5.1's Compress-Archive writes backslashes).
    Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::Open($zip, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($file in Get-ChildItem $stage -File) {
            [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file.FullName, "$portableName/$($file.Name)", [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
        }
    }
    finally {
        $archive.Dispose()
    }

    # Microsoft's WebView2 Runtime installer goes inside the setup program, for PCs without
    # the runtime. It must carry Microsoft's own signature.
    $bootstrapper = Join-Path $stage "MicrosoftEdgeWebview2Setup.exe"
    Invoke-WebRequest -Uri "https://go.microsoft.com/fwlink/p/?LinkId=2124703" -OutFile $bootstrapper -UseBasicParsing
    $signature = Get-AuthenticodeSignature $bootstrapper
    if ($signature.Status -ne "Valid" -or $signature.SignerCertificate.Subject -notmatch "O=Microsoft Corporation") {
        throw "The downloaded WebView2 installer is not signed by Microsoft."
    }

    $setup = Join-Path $OutDir "VaultKey-Setup-$version.exe"
    Remove-Item $setup -ErrorAction SilentlyContinue
    & $Iscc /Qp "/DAppVersion=$version" "/DAppFileVersion=$(($version -split '[-+]')[0])" "/DStageDir=$stage" "/DOutDir=$OutDir" (Join-Path $PSScriptRoot "installer.iss")
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed with exit code $LASTEXITCODE." }

    $sums = foreach ($file in @($setup, $zip)) {
        "{0}  {1}" -f (Get-FileHash $file -Algorithm SHA256).Hash.ToLower(), (Split-Path -Leaf $file)
    }
    # Plain ASCII with LF line ends, so `sha256sum -c` reads it anywhere.
    [System.IO.File]::WriteAllText((Join-Path $OutDir "SHA256SUMS.txt"), (($sums -join "`n") + "`n"))
    Write-Output "version $version"
    Get-ChildItem $OutDir | ForEach-Object { Write-Output ("{0,10}  {1}" -f $_.Length, $_.Name) }
}
finally {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}
