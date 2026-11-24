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

; ExtractTemporaryFile wants the name of the source file.
#define ExeName ExtractFileName(Exe)

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
Filename: "{app}\kaltstellen.exe"; Parameters: "tray"; Description: "{cm:LaunchTray}"; Flags: postinstall nowait skipifsilent

[UninstallRun]
; Quit resumes everything the tray froze; recover catches what a crashed
; tray left frozen; then the Run value goes.
Filename: "{app}\kaltstellen.exe"; Parameters: "quit"; Flags: runhidden waituntilterminated; RunOnceId: "quit"
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
var
  FirstInstall: Boolean;

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

{ Ends a running tray through its pipe before files are replaced, with the
  new executable: it speaks the protocol of every earlier version (v1) and
  works whether or not an older copy is installed. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
  Exe: String;
begin
  Result := '';
  ExtractTemporaryFile('{#ExeName}');
  Exe := ExpandConstant('{tmp}\{#ExeName}');
  { Exit code 5: the tray did not answer and did not end. Other codes come
    from a tray that ended (1: something could not be resumed, which it
    reported itself), so the files can be replaced. }
  if not Exec(Exe, 'quit', '', SW_HIDE, ewWaitUntilTerminated, Code) then
    Result := FmtMessage(CustomMessage('TrayStillRuns'), [SysErrorMessage(Code)])
  else if Code = 5 then
    Result := FmtMessage(CustomMessage('TrayStillRuns'), ['kaltstellen quit: exit code 5']);
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

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  ConfigDir, DataDir: String;
  Remove: Boolean;
begin
  if CurUninstallStep <> usPostUninstall then
    Exit;
  ConfigDir := KaltstellenDir('KALTSTELLEN_CONFIG_DIR', '{userappdata}');
  DataDir := KaltstellenDir('KALTSTELLEN_DATA_DIR', '{localappdata}');
  if not DirExists(ConfigDir) and not DirExists(DataDir) then
    Exit;
  { Keeping is the default: silent, or No in the question. /REMOVEDATA
    deletes without asking (tests, scripted removal). }
  if HasParam('/REMOVEDATA') then
    Remove := True
  else if UninstallSilent then
    Remove := False
  else
    Remove := SuppressibleMsgBox(FmtMessage(CustomMessage('RemoveData'), [ConfigDir, DataDir]),
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES;
  if Remove then
  begin
    DelTree(ConfigDir, True, True, True);
    DelTree(DataDir, True, True, True);
  end;
end;
