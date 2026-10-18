; Inno Setup script for i3win. Build with scripts\release.ps1, which passes
; /DAppVersion=<version from crates\i3win\Cargo.toml>.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6B1E2C4A-3F7D-4E8B-9A21-5C0D7E3F1A92}
AppName=i3win
AppVersion={#AppVersion}
AppVerName=i3win {#AppVersion}
AppPublisher=i3win
AppPublisherURL=https://github.com/behaviorism/i3win
VersionInfoVersion={#AppVersion}
; Per-user install: no admin rights needed.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\i3win
DisableProgramGroupPage=yes
DisableDirPage=auto
UninstallDisplayIcon={app}\i3win.exe
UninstallDisplayName=i3win
OutputDir=..\dist
OutputBaseFilename=i3win-setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; We stop i3win ourselves (gracefully, so hidden windows come back); never let
; the Restart Manager kill it.
CloseApplications=no
RestartApplications=no

[Tasks]
Name: "startup"; Description: "Start i3win when I sign in"

[Files]
Source: "..\target\release\i3win.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\i3win-status.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\i3win-msg.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\i3win"; Filename: "{app}\i3win.exe"
Name: "{autoprograms}\Exit i3win"; Filename: "{app}\i3win-msg.exe"; Parameters: "exit"; Flags: runminimized

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "i3win"; ValueData: """{app}\i3win.exe"""; Flags: uninsdeletevalue; Tasks: startup

[Run]
Filename: "{app}\i3win.exe"; Description: "Launch i3win now"; Flags: postinstall nowait skipifsilent

[Code]
function IsI3winRunning(): Boolean;
var
  Code: Integer;
begin
  { tasklist exits 0 either way; findstr sets the result. }
  Exec(ExpandConstant('{cmd}'), '/C tasklist /FI "IMAGENAME eq i3win.exe" /NH | findstr /I "i3win.exe" >nul',
    '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := (Code = 0);
end;

{ Asks a running i3win to exit cleanly (it shows every window it hid), then
  waits up to 5 s. Only if that fails is it force-killed; its next start would
  restore any windows left hidden. }
procedure StopI3win();
var
  Code, I: Integer;
  Msg: String;
begin
  if not IsI3winRunning() then
    exit;
  Msg := ExpandConstant('{app}\i3win-msg.exe');
  if FileExists(Msg) then
    Exec(Msg, 'exit', '', SW_HIDE, ewWaitUntilTerminated, Code);
  for I := 1 to 25 do
  begin
    if not IsI3winRunning() then
      exit;
    Sleep(200);
  end;
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM i3win.exe /F', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(500);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopI3win();
  Result := '';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    StopI3win();
end;
