; The Vault Key installer (Inno Setup 7). Built by packaging/package.ps1, which passes:
;   /DAppVersion=<version from Cargo.toml>
;   /DAppFileVersion=<its numeric part, major.minor.patch>
;   /DStageDir=<folder holding the files to install>
;   /DOutDir=<folder the setup program is written to>
;
; It installs for the current user only, so it never asks for administrator rights.

#ifndef AppVersion
  #error AppVersion is not set. Build the installer with packaging/package.ps1.
#endif
#ifndef StageDir
  #error StageDir is not set. Build the installer with packaging/package.ps1.
#endif
#ifndef AppFileVersion
  #define AppFileVersion AppVersion
#endif
#ifndef OutDir
  #define OutDir "."
#endif

#define AppName "Vault Key"
; The same publisher name as in the program's details (crates/app/build.rs).
#define AppPublisher "SInsanali"
#define AppUrl "https://github.com/SInsanali/vault-key"
#define AppExe "vault-key.exe"
; The same two names as in crates/app/src/platform.rs.
#define AppUserModelId "VaultKey.App"
#define AppRunningMutex "VaultKey.App.Running"
; The folder under %LOCALAPPDATA% where the app keeps settings, backups and caches
; (APP_FOLDER_NAME in crates/gkcore/src/app.rs), and the one it used when it was called
; Golden Key (OLD_APP_FOLDER_NAME). The app copies the old one's files into the new one.
#define AppDataFolder "VaultKey"
#define OldAppDataFolder "GoldenKey"

[Setup]
; Never change AppId: it is how a newer setup finds and upgrades an installed copy.
AppId={{3D860A2F-A8CD-433B-BF17-F4E790CDA03C}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
VersionInfoVersion={#AppFileVersion}.0
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
VersionInfoCompany={#AppPublisher}
VersionInfoCopyright=Copyright (C) 2026 {#AppPublisher}
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\{#AppName}
DisableProgramGroupPage=yes
DisableDirPage=yes
UsePreviousAppDir=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
AppMutex={#AppRunningMutex}
CloseApplications=no
SetupIconFile=..\crates\app\assets\vault-key.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir={#OutDir}
OutputBaseFilename=VaultKey-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#StageDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\WebView2Loader.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\THIRD_PARTY_NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
; Microsoft's small WebView2 Runtime installer. Only unpacked and run when the runtime is missing.
Source: "{#StageDir}\MicrosoftEdgeWebview2Setup.exe"; Flags: dontcopy

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; AppUserModelID: "{#AppUserModelId}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; AppUserModelID: "{#AppUserModelId}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
const
  WebView2ClientKey = 'Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  WebView2Download = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703';

function HasVersion(RootKey: Integer; const SubKey: String): Boolean;
var
  Version: String;
begin
  Result := RegQueryStringValue(RootKey, SubKey, 'pv', Version) and (Version <> '') and (Version <> '0.0.0.0');
end;

{ The WebView2 Runtime is part of Windows 11 and of current Windows 10, but not of every PC. }
function WebView2Installed: Boolean;
begin
  Result := HasVersion(HKLM32, 'SOFTWARE\' + WebView2ClientKey)
    or HasVersion(HKLM64, 'SOFTWARE\' + WebView2ClientKey)
    or HasVersion(HKCU, 'Software\' + WebView2ClientKey);
end;

procedure InstallWebView2;
var
  ExitCode: Integer;
  Started: Boolean;
begin
  WizardForm.StatusLabel.Caption := 'Installing the Microsoft Edge WebView2 Runtime...';
  ExtractTemporaryFile('MicrosoftEdgeWebview2Setup.exe');
  Started := Exec(ExpandConstant('{tmp}\MicrosoftEdgeWebview2Setup.exe'), '/silent /install', '', SW_HIDE, ewWaitUntilTerminated, ExitCode);
  if (not Started or not WebView2Installed) and not WizardSilent then
    MsgBox('{#AppName} needs the Microsoft Edge WebView2 Runtime, and it could not be installed.' + #13#10#13#10 +
      'Install it from ' + WebView2Download + ' and then start {#AppName}.', mbInformation, MB_OK);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and not WebView2Installed then
    InstallWebView2;
end;

{ True when the uninstaller was started with /DELETEDATA (for a silent uninstall). }
function DeleteDataSwitch: Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/DELETEDATA') = 0 then
      Result := True;
end;

{ Uninstalling removes the program, its shortcuts and its entry in Settings. The app's own
  data folder is kept unless the user asks: it holds the backups of their saves. The folder
  from before the rename goes with it, or the next install would copy it back. The game's
  save folder is never touched. }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir, OldDataDir, Folders: String;
  DeleteData: Boolean;
begin
  if CurUninstallStep <> usPostUninstall then
    exit;
  DataDir := ExpandConstant('{localappdata}\{#AppDataFolder}');
  OldDataDir := ExpandConstant('{localappdata}\{#OldAppDataFolder}');
  if not DirExists(DataDir) and not DirExists(OldDataDir) then
    exit;
  Folders := '';
  if DirExists(DataDir) then
    Folders := Folders + DataDir + #13#10;
  if DirExists(OldDataDir) then
    Folders := Folders + OldDataDir + #13#10;
  if UninstallSilent then
    DeleteData := DeleteDataSwitch
  else
    DeleteData := MsgBox('{#AppName} was removed.' + #13#10#13#10 +
      'Do you also want to delete its settings and the backups it made of your saves?' + #13#10 +
      Folders + #13#10 +
      'Your game saves are not touched either way.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES;
  if DeleteData then
  begin
    if DirExists(DataDir) then
      DelTree(DataDir, True, True, True);
    if DirExists(OldDataDir) then
      DelTree(OldDataDir, True, True, True);
  end;
end;
