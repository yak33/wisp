#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif
#ifndef MyAppExe
  #error MyAppExe must point to the release wisp.exe
#endif
#ifndef MyOutputDir
  #define MyOutputDir "."
#endif

#define MyAppName "Wisp"
#define MyAppPublisher "ZHANGCHAO"
#define MyAppUrl "https://github.com/yak33/wisp"
#define MyAppId "{{9A370750-EC8C-4DC7-B6CC-387C48D0674A}"

[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppUrl}
AppSupportURL={#MyAppUrl}
AppUpdatesURL={#MyAppUrl}/releases
DefaultDirName={localappdata}\Programs\Wisp
DefaultGroupName=Wisp
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#MyOutputDir}
OutputBaseFilename=Wisp-v{#MyAppVersion}-setup-win-x64
SetupIconFile=..\crates\app\assets\icons\app.ico
UninstallDisplayIcon={app}\wisp.exe
LicenseFile=..\LICENSE
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
CloseApplicationsFilter=wisp.exe
RestartApplications=no
VersionInfoVersion={#MyAppVersion}
VersionInfoCompany={#MyAppPublisher}
VersionInfoDescription=Wisp Windows installer
VersionInfoCopyright=Copyright (c) 2026 ZHANGCHAO
VersionInfoProductName={#MyAppName}

[Languages]
Name: "chinesesimp"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加快捷方式"; Flags: unchecked

[Files]
Source: "{#MyAppExe}"; DestDir: "{app}"; DestName: "wisp.exe"; Flags: ignoreversion
Source: "README-installed.txt"; DestDir: "{app}"; DestName: "README.txt"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Wisp"; Filename: "{app}\wisp.exe"
Name: "{group}\卸载 Wisp"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Wisp"; Filename: "{app}\wisp.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\wisp.exe"; Description: "启动 Wisp"; Flags: nowait postinstall skipifsilent

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  RegisteredCommand: string;
  InstalledCommand: string;
begin
  if CurUninstallStep <> usUninstall then
    Exit;

  InstalledCommand := '"' + ExpandConstant('{app}\wisp.exe') + '"';
  if RegQueryStringValue(HKCU, RunKey, 'Wisp', RegisteredCommand) and
     (CompareText(RegisteredCommand, InstalledCommand) = 0) then
    RegDeleteValue(HKCU, RunKey, 'Wisp');
end;
