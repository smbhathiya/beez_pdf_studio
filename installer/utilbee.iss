; Inno Setup script for Beez PDF Studio.
;
; Build (after `cargo build --release`):
;   ISCC.exe /DAppVersion=0.1.1 installer\utilbee.iss
; Output: dist\Beez-PDF-Studio-Setup-v<version>.exe
;
; Installs per user into %LOCALAPPDATA%\Programs (no administrator rights needed). This also
; keeps the in-app self-updater working, because the app can replace its own .exe there.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#define AppName "Beez PDF Studio"
#define AppExe "util_bee.exe"
#define AppUrl "https://github.com/smbhathiya/util_bee_pdf_studio"

[Setup]
; Never change AppId: Windows uses it to recognise upgrades of the same app.
AppId={{EA525DA2-E081-4D88-93A3-48039EFF08C5}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Beez
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
; Runs on 64-bit and 32-bit Windows (and Windows 10 on ARM through x86 emulation);
; the matching build of the app is installed.
ArchitecturesAllowed=x86compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=Beez-PDF-Studio-Setup-v{#AppVersion}
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "openwith"; Description: "Show {#AppName} in the ""Open with"" menu for PDF files"; GroupDescription: "Integration:"

[Files]
; 64-bit build (cargo build --release) and 32-bit build
; (cargo build --release --target i686-pc-windows-msvc).
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion; Check: Is64BitInstallMode
Source: "..\target\i686-pc-windows-msvc\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion; Check: not Is64BitInstallMode

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; "Open with" entry for .pdf files (per user; removed on uninstall).
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\DefaultIcon"; ValueType: string; ValueData: "{app}\{#AppExe},0"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithList\{#AppExe}"; ValueType: string; ValueData: ""; Flags: uninsdeletekey; Tasks: openwith

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
