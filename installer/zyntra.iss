; Zyntra Launcher installer (Inno Setup 6). Build with installer\build.ps1.
; Installs per user (no admin rights), so the launcher can keep zyntra.json next to the exe.

#define AppName "Zyntra Launcher"
#define AppVersion GetEnv("ZYNTRA_VERSION")
#define AppExe "zyntra.exe"

[Setup]
AppId={{6F1C2B9E-5A47-4C1D-9E2B-7A3D8C4F0E11}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Zyntra
DefaultDirName={localappdata}\Programs\Zyntra Launcher
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=ZyntraSetup-{#AppVersion}
SetupIconFile=..\dist\zyntra.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "desktopicon"; Description: "Создать ярлык на рабочем столе"; GroupDescription: "Ярлыки:"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\Удалить {#AppName}"; Filename: "{uninstallexe}"
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "Запустить {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Settings file the launcher creates. Game files (versions, worlds) stay where they are.
Type: files; Name: "{app}\zyntra.json"
