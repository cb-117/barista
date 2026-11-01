# Barista installer for Windows · https://github.com/cb-117/barista
#
#   irm https://baristatime.app/install.ps1 | iex
#
# Downloads the latest Barista Setup (a free, open-source app that keeps the PC awake while timers,
# commands, and AI agents work), checks it against the release's SHA-256, and runs it silently for
# the current user: it installs to %LOCALAPPDATA%\Programs\Barista, starts Barista when you sign in,
# adds it to your PATH and Start menu, connects Claude Code, Codex, and Cursor if they're set up for
# this user, and starts Barista in the notification area. Re-running updates it.
# No administrator rights needed. (For an all-users install, run Barista-Setup.exe yourself.)

# Everything runs from one function, called on the last line, so a download cut short runs nothing.
# Fallback for a latest release without Barista-Setup.exe: the portable zip, installed per user.
function Install-FromZip($base, $work) {
    $archive = 'barista-windows-x86_64.zip'
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$archive" -OutFile (Join-Path $work $archive)
    Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS.txt" -OutFile (Join-Path $work 'SHA256SUMS.txt')
    $expected = Get-Content (Join-Path $work 'SHA256SUMS.txt') |
        ForEach-Object { $parts = $_ -split '\s+'; if ($parts.Count -ge 2 -and $parts[1] -eq $archive) { $parts[0].ToLower() } } |
        Select-Object -First 1
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $work $archive)).Hash.ToLower()
    if (-not $expected -or $expected -ne $actual) {
        throw "The download didn't match its published checksum, so nothing was installed."
    }
    Expand-Archive -Path (Join-Path $work $archive) -DestinationPath (Join-Path $work 'unpacked')
    $source = Get-ChildItem (Join-Path $work 'unpacked') -Directory | Select-Object -First 1
    if (-not $source) { throw "The download was empty, so nothing was installed." }
    $destination = Join-Path $env:LOCALAPPDATA 'Programs\Barista'
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    Get-Process -Name 'barista-app' -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    Copy-Item -Path (Join-Path $source.FullName '*') -Destination $destination -Force
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $userPath) { $userPath = '' }
    if (($userPath -split ';') -notcontains $destination) {
        [Environment]::SetEnvironmentVariable('Path', ((($userPath.TrimEnd(';'), $destination) | Where-Object { $_ }) -join ';'), 'User')
    }
    if (($env:Path -split ';') -notcontains $destination) { $env:Path = "$env:Path;$destination" }
    Write-Host "Installed Barista to $destination"
    & (Join-Path $destination 'barista.exe') setup
}

function Install-Barista {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    $base = 'https://github.com/cb-117/barista/releases/latest/download'
    # x86_64 build; Windows on Arm runs it under emulation.
    $installer = 'Barista-Setup.exe'
    $work = Join-Path ([IO.Path]::GetTempPath()) ("barista-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        Write-Host 'Downloading Barista...'
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$base/$installer" -OutFile (Join-Path $work $installer)
        } catch {
            # Releases before 0.0.11 had no Setup program (GitHub answers 404): install the zip the way
            # earlier versions did. Any other failure (offline, a proxy, a 5xx) is reported as it is.
            $status = $null
            if ($_.Exception.Response) { $status = [int]$_.Exception.Response.StatusCode }
            if ($status -ne 404) { throw }
            Install-FromZip $base $work
            return
        }
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS.txt" -OutFile (Join-Path $work 'SHA256SUMS.txt')
        $expected = Get-Content (Join-Path $work 'SHA256SUMS.txt') |
            ForEach-Object { $parts = $_ -split '\s+'; if ($parts.Count -ge 2 -and $parts[1] -eq $installer) { $parts[0].ToLower() } } |
            Select-Object -First 1
        $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $work $installer)).Hash.ToLower()
        if (-not $expected -or $expected -ne $actual) {
            throw "The download didn't match its published checksum, so nothing was installed."
        }

        Write-Host 'Installing Barista...'
        $log = Join-Path $work 'install.log'
        # Not Start-Process -Wait: that waits for every process Setup starts, including Barista itself,
        # which keeps running. Wait for Setup alone (reading Handle keeps its exit code available).
        $setup = Start-Process -FilePath (Join-Path $work $installer) -PassThru -ArgumentList @(
            '/VERYSILENT', '/SUPPRESSMSGBOXES', '/CURRENTUSER', '/NORESTART', "/LOG=`"$log`"")
        $null = $setup.Handle
        $setup.WaitForExit()
        if ($setup.ExitCode -ne 0) {
            if (Test-Path $log) { Get-Content $log -Tail 20 | Write-Host }
            throw "Barista Setup failed (exit code $($setup.ExitCode))."
        }

        $destination = Join-Path $env:LOCALAPPDATA 'Programs\Barista'
        # Setup updated PATH for new terminals; make `barista` work in this one too.
        if (($env:Path -split ';') -notcontains $destination) { $env:Path = "$env:Path;$destination" }
        Write-Host "Installed Barista to $destination. Find the coffee cup in the notification area (click ^ if it's hidden)."
        Write-Host "Codex runs new hooks only after you trust them: open Codex, type /hooks, and trust Barista's hooks."
    } finally {
        Remove-Item -Recurse -Force -Path $work -ErrorAction SilentlyContinue
    }
}

Install-Barista
