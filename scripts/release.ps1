# Builds a Wyven release into dist\:
#   wyven-setup-<version>.exe   the installer (Inno Setup)
#   wyven-setup.exe             the same, with a stable name for download links
#   update.json                 what Wyven's update check reads (version, installer, SHA-256)
#
# Usage:  powershell -File scripts\release.ps1 [-Notes "What's new"]
# Publishing: put both files in one place (web server, GitHub release, network share) and set
# that update.json address in Wyven's settings (Updates → Update address).

param([string]$Notes = "")

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "Wyven $version"

Write-Host "Building (release) ..."
cargo build --release -p wyven-app
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found (winget install JRSoftware.InnoSetup)" }

Write-Host "Building the installer ..."
New-Item -ItemType Directory -Force dist | Out-Null
& $iscc /Q "/DAppVersion=$version" installer\wyven.iss
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed" }

$setup = "wyven-setup-$version.exe"
# A stable name for download links (the website links to wyven-setup.exe).
Copy-Item "dist\$setup" dist\wyven-setup.exe -Force
# Also next to the website, so its download button works (not in git, see .gitignore).
Copy-Item "dist\$setup" docs\wyven-setup.exe -Force
$hash = (Get-FileHash "dist\$setup" -Algorithm SHA256).Hash.ToLower()
$manifest = [ordered]@{ version = $version; url = $setup; sha256 = $hash; notes = $Notes }
# UTF-8 without BOM (Windows PowerShell's "utf8" would add one).
[IO.File]::WriteAllText("$root\dist\update.json", ($manifest | ConvertTo-Json), (New-Object Text.UTF8Encoding $false))

$size = [math]::Round((Get-Item "dist\$setup").Length / 1MB, 1)
Write-Host "dist\$setup ($size MB)"
Write-Host "dist\update.json (sha256 $hash)"
