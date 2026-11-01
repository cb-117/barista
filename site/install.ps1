# Barista installer for Windows · https://github.com/cb-117/barista
#
#   irm https://cb-117.github.io/barista/install.ps1 | iex
#
# Installs the latest Barista (a free, open-source app that keeps the PC awake while timers,
# commands, and AI agents work) to %LOCALAPPDATA%\Programs\Barista, adds it to your PATH and
# Start menu, then runs `barista setup`, which connects Claude Code and Codex if they're set up
# for this user and starts Barista in the notification area. Re-running updates it.
# No administrator rights needed.

# Everything runs from one function, called on the last line, so a download cut short runs nothing.
function Install-Barista {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    $base = 'https://github.com/cb-117/barista/releases/latest/download'
    # x86_64 build; Windows on Arm runs it under emulation.
    $archive = 'barista-windows-x86_64.zip'
    $work = Join-Path ([IO.Path]::GetTempPath()) ("barista-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        Write-Host 'Downloading Barista...'
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$base/$archive" -OutFile (Join-Path $work $archive)
        } catch {
            throw "Couldn't download $archive. Is there a Windows build in the latest release yet?"
        }
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
        # A running copy keeps its files locked; quit it so setup starts the new one.
        Get-Process -Name 'barista-app' -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Milliseconds 500
        Copy-Item -Path (Join-Path $source.FullName '*') -Destination $destination -Force

        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        if (-not $userPath) { $userPath = '' }
        if (($userPath -split ';') -notcontains $destination) {
            $updated = (($userPath.TrimEnd(';'), $destination) | Where-Object { $_ }) -join ';'
            [Environment]::SetEnvironmentVariable('Path', $updated, 'User')
            $env:Path = "$env:Path;$destination"
            Write-Host "Added $destination to your PATH. Open a new terminal to use the barista command."
        }

        $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut(
            (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Barista.lnk'))
        $shortcut.TargetPath = Join-Path $destination 'barista-app.exe'
        $shortcut.Description = 'Keeps your PC awake while work happens'
        $shortcut.Save()

        Write-Host "Installed Barista to $destination"
        & (Join-Path $destination 'barista.exe') setup
    } finally {
        Remove-Item -Recurse -Force -Path $work -ErrorAction SilentlyContinue
    }
}

Install-Barista
