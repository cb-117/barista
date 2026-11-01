# Builds Barista-Setup-<version>.exe with Inno Setup 6.3 or later (installing it with Chocolatey
# when the machine has none).
#
#   pwsh installer\windows\build-installer.ps1 -Version 0.0.11 -SourceDir rust\target\release -OutputDir dist
#
# Prints the path of the Setup program it built.

param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$SourceDir,
    [Parameter(Mandatory = $true)][string]$OutputDir
)

$ErrorActionPreference = 'Stop'

# ISCC's file version resource reads 0.0.0.0, so the version isn't checked here: Barista.iss refuses
# to compile on anything older than Inno Setup 6.3 (`#if Ver < EncodeVer(6, 3, 0)`).
function Find-Iscc {
    @((Get-Command iscc.exe -ErrorAction SilentlyContinue).Source,
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')) |
        Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}

$iscc = Find-Iscc
if (-not $iscc) {
    choco install innosetup -y --no-progress | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "choco install innosetup failed ($LASTEXITCODE)" }
    $iscc = Find-Iscc
    if (-not $iscc) { throw "Inno Setup 6 isn't installed" }
}
Write-Host "Using $iscc"

$script = Join-Path $PSScriptRoot 'Barista.iss'
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$source = (Resolve-Path $SourceDir).Path
$output = (Resolve-Path $OutputDir).Path
& $iscc /Qp "/DAppVersion=$Version" "/DSourceDir=$source" "/O$output" $script | Out-Host
if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }
$setup = Join-Path $output "Barista-Setup-$Version.exe"
if (-not (Test-Path $setup)) { throw "ISCC didn't produce $setup" }
$setup
