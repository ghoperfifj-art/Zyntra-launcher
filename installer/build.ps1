# Builds zyntra.exe in release mode and packs it into dist\ZyntraSetup-<version>.exe.
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# Version from Cargo.toml
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$env:ZYNTRA_VERSION = $version

# The .ico that build.rs generated from assets\logo.webp
New-Item -ItemType Directory -Force dist | Out-Null
$ico = Get-ChildItem target\release\build -Recurse -Filter zyntra.ico | Sort-Object LastWriteTime | Select-Object -Last 1
Copy-Item $ico.FullName dist\zyntra.ico -Force

$iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found (winget install JRSoftware.InnoSetup)" }
& $iscc /Q installer\zyntra.iss
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
Get-Item "dist\ZyntraSetup-$version.exe"
