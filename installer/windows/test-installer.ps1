# Smoke test for Barista Setup: fresh per-user install, upgrade over a running Barista, uninstall,
# and (when elevated) an all-users install and uninstall. Used by CI before anything is uploaded.
#
#   pwsh installer\windows\test-installer.ps1 -Setup installer-out\Barista-Setup-0.0.11.exe
#
# Changes this machine (installs and uninstalls Barista, touches the current user's Run key, PATH,
# and ~/.claude/settings.json in the all-users step): run it on a CI runner or a throwaway VM.

param(
    [Parameter(Mandatory = $true)][string]$Setup,
    [string]$Version
)

$ErrorActionPreference = 'Stop'
$Setup = (Resolve-Path $Setup).Path
if (-not $Version) {
    if ((Split-Path $Setup -Leaf) -match '^Barista-Setup-(.+)\.exe$') { $Version = $Matches[1] }
    else { throw "Pass -Version; it isn't in the file name $Setup" }
}
$logs = Join-Path ([IO.Path]::GetTempPath()) ("barista-installer-test-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $logs | Out-Null
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

function Fail($message) {
    Get-ChildItem $logs -Filter *.log -ErrorAction SilentlyContinue | ForEach-Object {
        Write-Host "---- $($_.Name) (last 40 lines)"
        Get-Content $_.FullName -Tail 40 | Write-Host
    }
    throw $message
}

# Never Start-Process -Wait: it waits for the whole process tree, and Setup starts Barista, which
# keeps running. Wait for Setup alone. (Reading Handle first keeps ExitCode available.)
function Invoke-Program($file, [string[]]$arguments, [int]$timeoutSeconds = 300) {
    $process = Start-Process -FilePath $file -ArgumentList $arguments -PassThru
    $null = $process.Handle
    if (-not $process.WaitForExit($timeoutSeconds * 1000)) {
        $process | Stop-Process -Force -ErrorAction SilentlyContinue
        Fail "$file didn't finish within $timeoutSeconds s"
    }
    return $process.ExitCode
}

function Install-Barista($scope, $log, $tasks = '!connectagents') {
    $code = Invoke-Program $Setup @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/NOLAUNCH', $scope,
        "/MERGETASKS=`"$tasks`"", "/LOG=`"$(Join-Path $logs $log)`"")
    if ($code -ne 0) { Fail "Setup $scope exited $code" }
}

# The uninstaller relaunches itself from a temporary copy, so its own exit says nothing: wait for
# the files to go.
function Uninstall-Barista($dir, $log) {
    $code = Invoke-Program (Join-Path $dir 'unins000.exe') @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART',
        "/LOG=`"$(Join-Path $logs $log)`"")
    if ($code -ne 0) { Fail "Uninstall exited $code" }
    for ($i = 0; $i -lt 90 -and (Test-Path (Join-Path $dir 'unins000.exe')); $i++) { Start-Sleep -Seconds 1 }
    foreach ($file in 'unins000.exe', 'barista-app.exe', 'barista.exe') {
        if (Test-Path (Join-Path $dir $file)) { Fail "Uninstall left $file behind" }
    }
    if (Get-ChildItem $dir -Filter *.old -ErrorAction SilentlyContinue) { Fail 'Uninstall left *.old files behind' }
}

function Assert-Installed($dir) {
    foreach ($file in 'barista.exe', 'barista-app.exe', 'LICENSE.txt', 'unins000.exe') {
        if (-not (Test-Path (Join-Path $dir $file))) { Fail "Not installed: $file" }
    }
    $reported = & (Join-Path $dir 'barista.exe') version
    if ($reported -ne "barista $Version") { Fail "barista version printed '$reported'" }
}

function Get-RunValue { (Get-ItemProperty $runKey -ErrorAction SilentlyContinue).Barista }

function Get-BaristaProcesses($dir) {
    @(Get-Process -Name barista-app, barista -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($dir, [StringComparison]::OrdinalIgnoreCase) })
}

# --- 1. Fresh per-user install --------------------------------------------------------------------
$dir = Join-Path $env:LOCALAPPDATA 'Programs\Barista'
Install-Barista '/CURRENTUSER' 'install.log'
Assert-Installed $dir
$expectedRun = '"' + (Join-Path $dir 'barista-app.exe') + '" --background'
if ((Get-RunValue) -ne $expectedRun) { Fail "Startup entry is '$(Get-RunValue)', expected '$expectedRun'" }
if (([Environment]::GetEnvironmentVariable('Path', 'User') -split ';') -notcontains $dir) { Fail "User PATH doesn't include $dir" }
$shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Barista.lnk'
if (-not (Test-Path $shortcut)) { Fail "No Start menu shortcut at $shortcut" }
if (Get-BaristaProcesses $dir) { Fail '/NOLAUNCH still started Barista' }
Write-Host 'Fresh install: files, version, startup entry, PATH, and shortcut verified.'

# --- 2. Upgrade over a running Barista, keeping the user's choices --------------------------------
$app = Start-Process -FilePath (Join-Path $dir 'barista-app.exe') -ArgumentList '--background' -PassThru
# `barista mcp` runs until its stdin closes; keep it open so the upgrade has to stop it.
$info = New-Object Diagnostics.ProcessStartInfo (Join-Path $dir 'barista.exe'), 'mcp --no-launch'
$info.UseShellExecute = $false
$info.RedirectStandardInput = $true
$info.RedirectStandardOutput = $true
$info.CreateNoWindow = $true
$mcp = [Diagnostics.Process]::Start($info)
Start-Sleep -Seconds 3
if ($mcp.HasExited) { Fail "barista mcp exited early ($($mcp.ExitCode))" }
if ($app.HasExited) { Write-Host "::warning::barista-app.exe exited early ($($app.ExitCode)); the upgrade only stops barista mcp." }
# The user turned Launch at login off since installing; an upgrade must not turn it back on.
Remove-ItemProperty $runKey -Name Barista -ErrorAction SilentlyContinue
# Asking for both per-user tasks must change nothing: an upgrade doesn't offer them.
Install-Barista '/CURRENTUSER' 'upgrade.log' 'startup,connectagents'
Assert-Installed $dir
if (-not $mcp.WaitForExit(5000)) { Fail 'The upgrade left barista mcp running' }
if (Get-BaristaProcesses $dir) { Fail 'The upgrade left Barista running' }
if ($null -ne (Get-RunValue)) { Fail "The upgrade wrote the startup entry again: '$(Get-RunValue)'" }
if (Select-String -Path (Join-Path $logs 'upgrade.log') -Pattern 'setup --no-launch' -Quiet) { Fail 'The upgrade ran barista setup' }
Write-Host 'Upgrade: running copies stopped and replaced, startup choice kept.'

# --- 3. Uninstall ---------------------------------------------------------------------------------
# Barista's own Launch at login writes the same value; uninstall removes it while it points here.
Set-ItemProperty $runKey -Name Barista -Value $expectedRun
Uninstall-Barista $dir 'uninstall.log'
if ($null -ne (Get-RunValue)) { Fail 'Uninstall left the startup entry' }
if (([Environment]::GetEnvironmentVariable('Path', 'User') -split ';') -contains $dir) { Fail 'Uninstall left the PATH entry' }
if (Test-Path $shortcut) { Fail 'Uninstall left the Start menu shortcut' }
Write-Host 'Uninstall: files, startup entry, PATH, and shortcut removed.'

# --- 4. All users (needs an elevated runner, as GitHub's are) ------------------------------------
$elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $elevated) {
    Write-Host '::warning::Not elevated: skipped the all-users install test.'
} else {
    $allDir = Join-Path $env:ProgramFiles 'Barista'
    Install-Barista '/ALLUSERS' 'install-allusers.log'
    Assert-Installed $allDir
    if ($null -ne (Get-RunValue)) { Fail "An all-users install wrote this user's startup entry" }
    $systemPath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    if (($systemPath -split ';') -notcontains $allDir) { Fail "System PATH doesn't include $allDir" }
    # Connect this user's Claude Code, as Barista would; the all-users uninstall must undo it.
    $claude = Join-Path $env:USERPROFILE '.claude\settings.json'
    $claudeBefore = if (Test-Path $claude) { Get-Content -Raw $claude } else { $null }
    & (Join-Path $allDir 'barista.exe') hooks install | Out-Host
    if ($LASTEXITCODE -ne 0) { Fail "barista hooks install exited $LASTEXITCODE" }
    & (Join-Path $allDir 'barista.exe') statusline install | Out-Host
    if ($LASTEXITCODE -ne 0) { Fail "barista statusline install exited $LASTEXITCODE" }
    if (-not (Select-String -Path $claude -Pattern 'barista' -Quiet)) { Fail "hooks install didn't connect $claude" }
    Uninstall-Barista $allDir 'uninstall-allusers.log'
    if (Select-String -Path $claude -Pattern 'barista' -Quiet) {
        Get-Content $claude | Write-Host
        Fail "The all-users uninstall left Barista in $claude"
    }
    if (([Environment]::GetEnvironmentVariable('Path', 'Machine') -split ';') -contains $allDir) { Fail 'Uninstall left the system PATH entry' }
    if ($null -eq $claudeBefore) { Remove-Item $claude -ErrorAction SilentlyContinue }
    Write-Host 'All users: Program Files install, system PATH, and every-profile disconnect verified.'
}

Remove-Item -Recurse -Force $logs -ErrorAction SilentlyContinue
Write-Host "Barista Setup $Version passed the install, upgrade, and uninstall checks."
