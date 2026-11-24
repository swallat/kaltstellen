; Per-user installer for kaltstellen (Inno Setup 6.7, spec Q2 "Installer").
;
; Built by scripts/installer.ps1, which passes:
;   /DVersion=<version>          what the program reports, e.g. 1.0.0-beta.2
;   /DNumericVersion=<a.b.c.d>   the same as four numbers (file version)
;   /DArch=x86_64|aarch64        std::env::consts::ARCH of the executable
;   /DExe=<path>                 the executable to install
;   /DOutputDir=<dir> /DOutputName=<name without .exe>
;
; No administrator rights, ever: it installs below %LOCALAPPDATA%\Programs,
; where kaltstellen can still replace itself when it updates (UP1).

#ifndef Version
  #error Version is missing (/DVersion=...)
#endif
#ifndef NumericVersion
  #error NumericVersion is missing (/DNumericVersion=...)
#endif
#ifndef Arch
  #error Arch is missing (/DArch=x86_64 or /DArch=aarch64)
#endif
#ifndef Exe
  #error Exe is missing (/DExe=...)
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif
#ifndef OutputName
  #define OutputName "kaltstellen-" + Version + "-windows-" + Arch + "-setup"
#endif

#if Arch == "x86_64"
  #define Architectures "x64compatible"
#elif Arch == "aarch64"
  #define Architectures "arm64"
#else
  #error Arch must be x86_64 or aarch64
#endif

[Setup]
; Never change the AppId: the uninstall entry and the updater
; (src/update/uninstall_entry.rs) find the installation by it.
AppId={{0867F4F2-B652-4569-9343-104E8470E95F}
AppName=kaltstellen
AppVersion={#Version}
AppVerName=kaltstellen {#Version}
AppPublisher=kaltstellen
AppPublisherURL=https://github.com/swallat/kaltstellen
AppSupportURL=https://github.com/swallat/kaltstellen/issues
AppUpdatesURL=https://github.com/swallat/kaltstellen/releases
VersionInfoVersion={#NumericVersion}
VersionInfoProductVersion={#NumericVersion}
VersionInfoProductTextVersion={#Version}
VersionInfoDescription=kaltstellen setup
PrivilegesRequired=lowest
DefaultDirName={userpf}\kaltstellen
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=no
UsePreviousAppDir=yes
UsePreviousTasks=yes
ArchitecturesAllowed={#Architectures}
ArchitecturesInstallIn64BitMode={#Architectures}
; The tray must be quit through its pipe, which resumes what it froze; the
; Restart Manager would just close it.
CloseApplications=no
RestartApplications=no
SetupIconFile=..\branding\app\kaltstellen.ico
UninstallDisplayIcon={app}\kaltstellen.exe
UninstallDisplayName=kaltstellen
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"

[CustomMessages]
en.AutostartTask=Start kaltstellen when I sign in to Windows
de.AutostartTask=kaltstellen bei der Anmeldung an Windows starten
en.LaunchTray=Start kaltstellen now
de.LaunchTray=kaltstellen jetzt starten
en.TrayStillRuns=kaltstellen is still running and could not be ended:%n%n%1%n%nQuit it from its tray menu and run the setup again.
de.TrayStillRuns=kaltstellen läuft noch und konnte nicht beendet werden:%n%n%1%n%nBeende es über das Menü im Infobereich und starte das Setup erneut.
en.RemoveData=Also delete your kaltstellen profiles, settings and data?%n%n%1%n%2%n%nChoose No to keep them for a later installation.
de.RemoveData=Auch deine kaltstellen-Profile, Einstellungen und Daten löschen?%n%n%1%n%2%n%nWähle Nein, um sie für eine spätere Installation zu behalten.

[CustomMessages]
en.WindowStillOpen=The kaltstellen window is still open and did not close. Close it and run this again.
de.WindowStillOpen=Das kaltstellen-Fenster ist noch offen und hat sich nicht geschlossen. Schließe es und starte dies erneut.

[Tasks]
Name: "autostart"; Description: "{cm:AutostartTask}"

[Files]
Source: "{#Exe}"; DestDir: "{app}"; DestName: "kaltstellen.exe"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\kaltstellen"; Filename: "{app}\kaltstellen.exe"; Parameters: "tray"; WorkingDir: "{app}"

[Run]
; Start at sign-in is set on a first installation only; an upgrade keeps
; what was chosen since (main window, `kaltstellen autostart`).
Filename: "{app}\kaltstellen.exe"; Parameters: "autostart on"; Flags: runhidden waituntilterminated; Tasks: autostart; Check: IsFirstInstall
Filename: "{app}\kaltstellen.exe"; Parameters: "autostart off"; Flags: runhidden waituntilterminated; Tasks: not autostart; Check: IsFirstInstall
; A silent run starts the tray again in CurStepChanged when one was running.
Filename: "{app}\kaltstellen.exe"; Parameters: "tray"; Description: "{cm:LaunchTray}"; Flags: postinstall nowait skipifsilent

[UninstallRun]
; The tray was quit after the confirmation (usAppMutexCheck, which stops when it does not
; end). recover catches what a crashed tray left frozen; then the Run value
; goes.
Filename: "{app}\kaltstellen.exe"; Parameters: "recover"; Flags: runhidden waituntilterminated; RunOnceId: "recover"
Filename: "{app}\kaltstellen.exe"; Parameters: "autostart off"; Flags: runhidden waituntilterminated; RunOnceId: "autostart"

[UninstallDelete]
; What the updater leaves next to the executable (src/update/install.rs).
Type: files; Name: "{app}\kaltstellen.old.exe"
Type: files; Name: "{app}\kaltstellen.old-*.exe"
Type: files; Name: "{app}\kaltstellen.new.exe"
Type: files; Name: "{app}\kaltstellen.failed.exe"
Type: dirifempty; Name: "{app}"

[Code]
const
  WM_CLOSE = $0010;
  { src/win32/webview.rs CLASS: the main window, a process of its own that
    keeps running from the installed kaltstellen.exe after the tray ended. }
  MainWindowClass = 'kaltstellen-main';
  { Exit code of `kaltstellen quit` when the tray neither answered nor
    ended (src/exit.rs UNREACHABLE). }
  QuitUnreachable = 5;

var
  FirstInstall: Boolean;
  TrayWasRunning: Boolean;

function IsFirstInstall: Boolean;
begin
  Result := FirstInstall;
end;

function InitializeSetup: Boolean;
begin
  FirstInstall := not RegKeyExists(HKCU,
    'Software\Microsoft\Windows\CurrentVersion\Uninstall\{0867F4F2-B652-4569-9343-104E8470E95F}_is1');
  Result := True;
end;

{ Closes the main window, which would hold the executable open; true when
  none is left after 15 s. }
function CloseMainWindow: Boolean;
var
  Wnd: HWND;
  Waited: Integer;
begin
  Waited := 0;
  Wnd := FindWindowByClassName(MainWindowClass);
  while (Wnd <> 0) and (Waited < 15000) do
  begin
    PostMessage(Wnd, WM_CLOSE, 0, 0);
    Sleep(250);
    Waited := Waited + 250;
    Wnd := FindWindowByClassName(MainWindowClass);
  end;
  Result := Wnd = 0;
end;

{ `kaltstellen quit` with Exe: ends a running tray through its pipe, which
  resumes everything it froze. Returns '' or why it did not work; Running
  says whether there was a tray. }
function QuitTray(const Exe: String; var Running: Boolean): String;
var
  Code, I: Integer;
  Output: TExecOutput;
begin
  Result := '';
  Running := False;
  if not ExecAndCaptureOutput(Exe, 'quit', '', SW_HIDE, ewWaitUntilTerminated, Code, Output) then
  begin
    Result := FmtMessage(CustomMessage('TrayStillRuns'), [SysErrorMessage(Code)]);
    Exit;
  end;
  { Other codes come from a tray that ended (1: something could not be
    resumed, which it reported itself), so the files can be replaced. }
  if Code = QuitUnreachable then
  begin
    Result := FmtMessage(CustomMessage('TrayStillRuns'), ['kaltstellen quit: exit code 5']);
    Exit;
  end;
  Running := True;
  for I := 0 to GetArrayLength(Output.StdOut) - 1 do
    if Trim(Output.StdOut[I]) = 'no tray runs' then
      Running := False;
end;

{ Before files are replaced: the window closes and the tray ends, with the
  new executable (it speaks the pipe protocol of every earlier version,
  v1, and works whether or not an older copy is installed). }
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  if not CloseMainWindow then
  begin
    Result := CustomMessage('WindowStillOpen');
    Exit;
  end;
  ExtractTemporaryFile('kaltstellen.exe');
  Result := QuitTray(ExpandConstant('{tmp}\kaltstellen.exe'), TrayWasRunning);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Code: Integer;
begin
  { Interactively the last page offers to start it; a silent upgrade puts
    back the tray it ended. }
  if (CurStep = ssPostInstall) and TrayWasRunning and WizardSilent then
    Exec(ExpandConstant('{app}\kaltstellen.exe'), 'tray', '', SW_HIDE, ewNoWait, Code);
end;

{ After the user confirmed the uninstall (usAppMutexCheck comes after the
  question, before anything is removed): the window closes and the tray
  ends. When that fails the uninstall stops, so the program is never
  removed under a tray that still holds programs frozen. }
procedure EndTrayForUninstall;
var
  Why: String;
  Running: Boolean;
begin
  Why := '';
  if not CloseMainWindow then
    Why := CustomMessage('WindowStillOpen')
  else if FileExists(ExpandConstant('{app}\kaltstellen.exe')) then
    Why := QuitTray(ExpandConstant('{app}\kaltstellen.exe'), Running);
  if Why <> '' then
  begin
    if not UninstallSilent then
      MsgBox(Why, mbError, MB_OK);
    RaiseException(Why);
  end;
end;

function HasParam(const Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), Name) = 0 then
      Result := True;
end;

{ The same folders as src/paths.rs. }
function KaltstellenDir(const OverrideVar, BaseConstant: String): String;
begin
  Result := GetEnv(OverrideVar);
  if Result = '' then
    Result := ExpandConstant(BaseConstant) + '\kaltstellen';
end;

{ Only what kaltstellen writes there, then the folder if it is empty: the
  folders can be redirected (KALTSTELLEN_CONFIG_DIR, KALTSTELLEN_DATA_DIR)
  to one that holds other files too. }
procedure DeleteFiles(const Dir, Pattern: String);
begin
  DelTree(AddBackslash(Dir) + Pattern, False, True, False);
end;

procedure RemoveConfig(const Dir: String);
begin
  DeleteFiles(Dir, 'config.toml');
  DeleteFiles(Dir, 'config.toml.*');
  DeleteFiles(Dir, 'settings.json');
  DeleteFiles(Dir, 'settings.json.*');
  DeleteFiles(Dir, 'settings.lock');
  RemoveDir(Dir);
end;

procedure RemoveData(const Dir: String);
begin
  DeleteFiles(Dir, 'state.json');
  DeleteFiles(Dir, 'state.json.*');
  DeleteFiles(Dir, 'state.lock');
  DeleteFiles(Dir, 'seen.json');
  DeleteFiles(Dir, 'seen.json.*');
  DeleteFiles(Dir, 'update.json');
  DeleteFiles(Dir, 'update.json.*');
  DeleteFiles(Dir, 'kaltstellen.log');
  DeleteFiles(Dir, 'kaltstellen.log.*');
  { The WebView2 data of the main window (src/gui/mod.rs). }
  DelTree(AddBackslash(Dir) + 'webview2', True, True, True);
  RemoveDir(Dir);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  ConfigDir, DataDir: String;
  Remove: Boolean;
begin
  if CurUninstallStep = usAppMutexCheck then
    EndTrayForUninstall;
  if CurUninstallStep <> usPostUninstall then
    Exit;
  ConfigDir := KaltstellenDir('KALTSTELLEN_CONFIG_DIR', '{userappdata}');
  DataDir := KaltstellenDir('KALTSTELLEN_DATA_DIR', '{localappdata}');
  { Without profiles there is nothing to keep (the commands above create
    the folders anyway); with them keeping is the default: silent, or No
    in the question. /REMOVEDATA deletes without asking (tests, scripts). }
  if HasParam('/REMOVEDATA') or not FileExists(AddBackslash(ConfigDir) + 'config.toml') then
    Remove := True
  else if UninstallSilent then
    Remove := False
  else
    Remove := SuppressibleMsgBox(FmtMessage(CustomMessage('RemoveData'), [ConfigDir, DataDir]),
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES;
  if Remove then
  begin
    RemoveConfig(ConfigDir);
    RemoveData(DataDir);
  end;
end;
