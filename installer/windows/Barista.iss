; Barista for Windows · Inno Setup 6 script
;
; Builds Barista-Setup-<version>.exe from the Rust release binaries:
;
;   ISCC.exe /DAppVersion=0.0.11 /DSourceDir=C:\path\to\rust\target\release installer\windows\Barista.iss
;
; Installs for the current user by default (%LOCALAPPDATA%\Programs\Barista, no administrator
; rights); the first page also offers "all users" (Program Files, asks for elevation). Command line:
; /CURRENTUSER or /ALLUSERS, /VERYSILENT, /MERGETASKS="!connectagents", and so on.
;
; Barista-specific command line: /NOLAUNCH skips starting Barista at the end (used by CI).
;
; Optional tasks (all but the desktop icon checked by default):
;   startup        HKCU Run value "Barista" = "<app>\barista-app.exe" --background (exactly what the
;                  app's own Launch at login setting writes, so the two stay in agreement)
;   addtopath      adds the install folder to PATH (user PATH, or system PATH for all-users installs)
;   connectagents  runs `barista setup --no-launch` to connect Claude Code, Codex, and Cursor
;   desktopicon    a desktop shortcut
; startup and connectagents are per-user choices: they're offered only on a fresh per-user install.
; Upgrades keep whatever the user chose since (Launch at login, connected agents), and an all-users
; install leaves each user's startup entry and agents to Barista itself.
;
; Uninstalling first disconnects Barista's agent hooks and Claude Code status line (so agents aren't
; left calling a deleted program) — for every user profile when it was an all-users install — then
; removes the files, the startup entry (if it points here), and the PATH entry.
; Barista's data (%LOCALAPPDATA%\Barista, %APPDATA%\Barista) is left in place.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\rust\target\release"
#endif
#ifndef LicensePath
  #define LicensePath "..\..\LICENSE"
#endif
#if Ver < EncodeVer(6, 3, 0)
  #error Barista.iss needs Inno Setup 6.3 or later.
#endif

[Setup]
; Never change AppId: it's how upgrades and the uninstaller find an existing install.
AppId={{8F3C2A1E-6B4D-4E7A-9C51-2D7F0B3A9E64}
AppName=Barista
AppVersion={#AppVersion}
AppVerName=Barista {#AppVersion}
AppPublisher=cb-117
AppPublisherURL=https://baristatime.app
AppSupportURL=https://github.com/cb-117/barista/issues
AppUpdatesURL=https://github.com/cb-117/barista/releases
AppComments=Keeps your PC awake while timers, commands, and AI agents work.
VersionInfoVersion={#AppVersion}
VersionInfoDescription=Barista Setup
DefaultDirName={autopf}\Barista
DefaultGroupName=Barista
DisableProgramGroupPage=yes
DisableDirPage=auto
; Current user by default; the dialog (or /ALLUSERS) can choose an all-users install instead.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
; The startup entry is per user even in an all-users install, matching the app's own setting.
UsedUserAreasWarning=no
MinVersion=10.0
; x64compatible needs Inno Setup 6.3 or later (checked above).
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
SetupIconFile=barista.ico
UninstallDisplayIcon={app}\barista-app.exe
UninstallDisplayName=Barista
OutputBaseFilename=Barista-Setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
ChangesEnvironment=yes
; Barista is stopped in [Code] instead (a tray app has no window for Restart Manager to close).
CloseApplications=no
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "startup"; Description: "Start Barista when I sign in"; Check: OffersUserChoices
Name: "addtopath"; Description: "Add Barista to PATH (for the barista command)"
Name: "connectagents"; Description: "Connect Claude Code, Codex, and Cursor"; Check: OffersUserChoices
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[InstallDelete]
; Copies set aside by an earlier install or upgrade because they were still running.
Type: files; Name: "{app}\*.old"

[Files]
Source: "{#SourceDir}\barista.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\barista-app.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#LicensePath}"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Barista"; Filename: "{app}\barista-app.exe"; Comment: "Keeps your PC awake while work happens"
Name: "{autodesktop}\Barista"; Filename: "{app}\barista-app.exe"; Comment: "Keeps your PC awake while work happens"; Tasks: desktopicon

[Registry]
; The uninstaller removes it in [Code], and only while it still points into this install.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Barista"; ValueData: """{app}\barista-app.exe"" --background"; Tasks: startup

[Run]
; Connects the agents' hooks and Claude Code's status line without starting Barista (the next entry
; does that), so it exits non-zero after connecting; Setup ignores the exit code.
Filename: "{app}\barista.exe"; Parameters: "setup --no-launch"; Flags: runhidden waituntilterminated runasoriginaluser; Tasks: connectagents; StatusMsg: "Connecting Claude Code, Codex, and Cursor..."
; Runs after silent installs too (no skipifsilent), so the one-line installer ends with Barista running.
Filename: "{app}\barista-app.exe"; Parameters: "{code:LaunchParameters}"; Description: "Start Barista now"; Flags: nowait postinstall runasoriginaluser; Check: ShouldLaunch

[UninstallRun]
; Per-user installs: this user's agents. An all-users uninstall disconnects every profile in [Code].
Filename: "{app}\barista.exe"; Parameters: "hooks uninstall"; Flags: runhidden waituntilterminated; RunOnceId: "DisconnectClaudeCode"; Check: not IsAdminInstallMode
Filename: "{app}\barista.exe"; Parameters: "hooks uninstall --agent codex"; Flags: runhidden waituntilterminated; RunOnceId: "DisconnectCodex"; Check: not IsAdminInstallMode
Filename: "{app}\barista.exe"; Parameters: "hooks uninstall --agent cursor"; Flags: runhidden waituntilterminated; RunOnceId: "DisconnectCursor"; Check: not IsAdminInstallMode
Filename: "{app}\barista.exe"; Parameters: "statusline uninstall"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveStatusLine"; Check: not IsAdminInstallMode

[UninstallDelete]
Type: files; Name: "{app}\*.old"

[Code]
const
  UserEnvironmentKey = 'Environment';
  SystemEnvironmentKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{8F3C2A1E-6B4D-4E7A-9C51-2D7F0B3A9E64}_is1';
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  ProfileListKey = 'SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList';

var
  Upgrading: Boolean;

function SetEnvironmentVariable(lpName, lpValue: String): BOOL;
  external 'SetEnvironmentVariableW@kernel32.dll stdcall';

{ The user PATH for per-user installs, the system PATH for all-users installs. }
function EnvironmentRoot: Integer;
begin
  if IsAdminInstallMode then
    Result := HKEY_LOCAL_MACHINE
  else
    Result := HKEY_CURRENT_USER;
end;

function EnvironmentKey: String;
begin
  if IsAdminInstallMode then
    Result := SystemEnvironmentKey
  else
    Result := UserEnvironmentKey;
end;

{ An upgrade is found by the uninstall entry an earlier install left in the same root. Read once,
  before Setup writes its own. }
procedure InitializeWizard;
begin
  if IsAdminInstallMode then
    Upgrading := RegKeyExists(HKEY_LOCAL_MACHINE, UninstallKey)
  else
    Upgrading := RegKeyExists(HKEY_CURRENT_USER, UninstallKey);
end;

{ "Start Barista when I sign in" and "Connect…" are offered only on a fresh per-user install. }
function OffersUserChoices: Boolean;
begin
  Result := not IsAdminInstallMode and not Upgrading;
end;

function ShouldLaunch: Boolean;
begin
  Result := Pos('/NOLAUNCH', Uppercase(GetCmdTail)) = 0;
end;

{ A fresh per-user install with "Start Barista when I sign in" unchecked: tell Barista the user
  already chose, so its first run doesn't turn Launch at login on anyway. }
function LaunchParameters(Param: String): String;
begin
  if OffersUserChoices and not WizardIsTaskSelected('startup') then
    Result := '--login-choice-made'
  else
    Result := '';
end;

function SamePath(A, B: String): Boolean;
begin
  Result := CompareText(RemoveBackslashUnlessRoot(Trim(A)), RemoveBackslashUnlessRoot(Trim(B))) = 0;
end;

function PathHas(Paths, Dir: String): Boolean;
var
  Rest, Part: String;
  P: Integer;
begin
  Result := False;
  Rest := Paths;
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P > 0 then
    begin
      Part := Copy(Rest, 1, P - 1);
      Delete(Rest, 1, P);
    end
    else
    begin
      Part := Rest;
      Rest := '';
    end;
    if (Part <> '') and SamePath(Part, Dir) then
    begin
      Result := True;
      Exit;
    end;
  end;
end;

procedure AddToPath(Dir: String);
var
  Paths: String;
begin
  if not RegQueryStringValue(EnvironmentRoot, EnvironmentKey, 'Path', Paths) then
    Paths := '';
  if PathHas(Paths, Dir) then
    Exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then
    Paths := Paths + ';';
  Paths := Paths + Dir;
  RegWriteExpandStringValue(EnvironmentRoot, EnvironmentKey, 'Path', Paths);
end;

procedure RemoveFromPath(Dir: String);
var
  Paths, Rest, Part, Kept: String;
  P: Integer;
begin
  if not RegQueryStringValue(EnvironmentRoot, EnvironmentKey, 'Path', Paths) then
    Exit;
  Rest := Paths;
  Kept := '';
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P > 0 then
    begin
      Part := Copy(Rest, 1, P - 1);
      Delete(Rest, 1, P);
    end
    else
    begin
      Part := Rest;
      Rest := '';
    end;
    if (Part <> '') and not SamePath(Part, Dir) then
    begin
      if Kept <> '' then
        Kept := Kept + ';';
      Kept := Kept + Part;
    end;
  end;
  if Kept <> Paths then
    RegWriteExpandStringValue(EnvironmentRoot, EnvironmentKey, 'Path', Kept);
end;

{ Stops every barista-app.exe and barista.exe (the tray app, a `barista mcp` server, a hook) running
  from this install folder and waits up to 10 seconds for them to exit; copies installed elsewhere
  are left alone. True once none is left. Awake sessions end, as they would on quit. }
function StopBarista: Boolean;
var
  Dir, Script: String;
  ResultCode: Integer;
begin
  Dir := AddBackslash(ExpandConstant('{app}'));
  StringChangeEx(Dir, '''', '''''', True);
  Script :=
    '$d = ''' + Dir + '''; ' +
    '$p = @(Get-Process -Name barista-app, barista -ErrorAction SilentlyContinue | ' +
    'Where-Object { $_.Path -and $_.Path.StartsWith($d, [StringComparison]::OrdinalIgnoreCase) }); ' +
    '$p | Stop-Process -Force -ErrorAction SilentlyContinue; ' +
    '$p | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue; ' +
    'if (@($p | Where-Object { -not $_.HasExited }).Count) { exit 1 }';
  Result := Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "' + Script + '"',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode) and (ResultCode = 0);
end;

{ Windows lets a running program be renamed but not replaced: move one that's still running out of
  the way so Setup can install the new copy. The .old file goes on the next install or uninstall. }
procedure SetAside(Name: String);
var
  Path, Aside: String;
  N: Integer;
begin
  Path := ExpandConstant('{app}\') + Name;
  if not FileExists(Path) then
    Exit;
  Aside := Path + '.old';
  N := 1;
  while FileExists(Aside) and not DeleteFile(Aside) do
  begin
    Aside := Path + '.' + IntToStr(N) + '.old';
    N := N + 1;
  end;
  if RenameFile(Path, Aside) then
    Log('Set aside ' + Path + ' as ' + Aside)
  else
    Log('Couldn''t set aside ' + Path);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  if not StopBarista then
  begin
    Log('Barista was still running after 10 seconds.');
    SetAside('barista-app.exe');
    SetAside('barista.exe');
  end;
  Result := '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));
end;

function InitializeUninstall(): Boolean;
begin
  if not StopBarista then
    Log('Barista was still running after 10 seconds; its files may be left behind.');
  Result := True;
end;

{ Real user profiles: local and domain accounts (S-1-5-21-...) and Microsoft Entra ID accounts
  (S-1-12-1-...). The system profiles (S-1-5-18, -19, -20) are skipped. }
function IsUserSid(Sid: String): Boolean;
begin
  Result := (Pos('S-1-5-21-', Sid) = 1) or (Pos('S-1-12-1-', Sid) = 1);
end;

procedure RunHidden(Exe, Parameters: String);
var
  ResultCode: Integer;
begin
  if Exec(Exe, Parameters, '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    Log(Exe + ' ' + Parameters + ' exited ' + IntToStr(ResultCode))
  else
    Log('Couldn''t run ' + Exe + ' ' + Parameters);
end;

{ Any user of an all-users install may have connected their agents: disconnect Claude Code, Codex,
  Cursor, and the Claude Code status line in every profile where their settings exist.
  BARISTA_DATA_DIR points `barista` at that user's data, where the status line backup lives. }
procedure DisconnectEveryProfile;
var
  Sids: TArrayOfString;
  I: Integer;
  Exe, Profile, Claude, Codex, Cursor: String;
begin
  Exe := ExpandConstant('{app}\barista.exe');
  if not FileExists(Exe) then
    Exit;
  if not RegGetSubkeyNames(HKEY_LOCAL_MACHINE, ProfileListKey, Sids) then
    Exit;
  for I := 0 to GetArrayLength(Sids) - 1 do
  begin
    if not IsUserSid(Sids[I]) then
      Continue;
    if not RegQueryStringValue(HKEY_LOCAL_MACHINE, ProfileListKey + '\' + Sids[I], 'ProfileImagePath', Profile) then
      Continue;
    StringChangeEx(Profile, '%SystemDrive%', GetEnv('SystemDrive'), True);
    StringChangeEx(Profile, '%SystemRoot%', GetEnv('SystemRoot'), True);
    Profile := RemoveBackslashUnlessRoot(Profile);
    if not DirExists(Profile) then
      Continue;
    SetEnvironmentVariable('BARISTA_DATA_DIR', Profile + '\AppData\Local\Barista');
    Claude := Profile + '\.claude\settings.json';
    Codex := Profile + '\.codex\hooks.json';
    Cursor := Profile + '\.cursor\hooks.json';
    if FileExists(Claude) then
    begin
      RunHidden(Exe, 'hooks uninstall --settings "' + Claude + '"');
      RunHidden(Exe, 'statusline uninstall --settings "' + Claude + '"');
    end;
    if FileExists(Codex) then
      RunHidden(Exe, 'hooks uninstall --agent codex --settings "' + Codex + '"');
    if FileExists(Cursor) then
      RunHidden(Exe, 'hooks uninstall --agent cursor --settings "' + Cursor + '"');
  end;
  { An empty value counts as unset (barista ignores an empty BARISTA_DATA_DIR). }
  SetEnvironmentVariable('BARISTA_DATA_DIR', '');
end;

{ The startup entry, whether Setup or Barista's own Launch at login wrote it, but only while it
  points into this install (a copy installed somewhere else may own it now). }
procedure RemoveStartupEntry;
var
  Value: String;
begin
  if RegQueryStringValue(HKEY_CURRENT_USER, RunKey, 'Barista', Value) and
     (Pos(Lowercase(AddBackslash(ExpandConstant('{app}'))), Lowercase(Value)) > 0) then
    RegDeleteValue(HKEY_CURRENT_USER, RunKey, 'Barista');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    if IsAdminInstallMode then
      DisconnectEveryProfile;
    RemoveFromPath(ExpandConstant('{app}'));
    RemoveStartupEntry;
  end;
end;
