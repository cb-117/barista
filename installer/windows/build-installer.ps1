# Builds Barista-Setup-<version>.exe with Inno Setup 6.3 or later (installing a pinned 6.x with
# Chocolatey when the machine has none, or only an older one).
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
$pinned = '6.4.3'
$minimum = [Version]'6.3'

function Find-Iscc {
    $candidates = @((Get-Command iscc.exe -ErrorAction SilentlyContinue).Source,
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')) | Where-Object { $_ -and (Test-Path $_) }
    foreach ($candidate in $candidates) {
        $version = [Version](Get-Item $candidate).VersionInfo.FileVersion.Split(' ')[0]
        if ($version -ge $minimum) { return $candidate }
        Write-Host "Ignoring Inno Setup $version at $candidate (Barista.iss needs $minimum or later)."
    }
    return $null
}

$iscc = Find-Iscc
if (-not $iscc) {
    choco install innosetup --version=$pinned --allow-downgrade -y --no-progress | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "choco install innosetup $pinned failed ($LASTEXITCODE)" }
    $iscc = Find-Iscc
    if (-not $iscc) { throw "Inno Setup $minimum or later isn't installed" }
}
Write-Host "Using $iscc ($((Get-Item $iscc).VersionInfo.FileVersion))"

$script = Join-Path $PSScriptRoot 'Barista.iss'
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$source = (Resolve-Path $SourceDir).Path
$output = (Resolve-Path $OutputDir).Path
& $iscc /Qp "/DAppVersion=$Version" "/DSourceDir=$source" "/O$output" $script | Out-Host
if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }
$setup = Join-Path $output "Barista-Setup-$Version.exe"
if (-not (Test-Path $setup)) { throw "ISCC didn't produce $setup" }
$setup
