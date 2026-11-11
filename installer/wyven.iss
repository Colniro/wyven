; Wyven setup (Inno Setup 6). Built by scripts/release.ps1, which passes the version:
;   ISCC.exe /DAppVersion=0.1.0 installer\wyven.iss
; Installs for the current user (no admin rights): %LOCALAPPDATA%\Programs\Wyven.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{7B2C7E5A-3F4D-4E8B-9C21-5A6D2F8E1B37}
AppName=Wyven
AppVersion={#AppVersion}
AppVerName=Wyven {#AppVersion}
AppPublisher=Wyven
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Wyven
DefaultGroupName=Wyven
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir=..\dist
OutputBaseFilename=wyven-setup-{#AppVersion}
SetupIconFile=..\assets\wyven.ico
UninstallDisplayIcon={app}\wyven.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; The updater runs the setup while Wyven is open: close it and start it again afterwards.
CloseApplications=yes
RestartApplications=yes
ChangesEnvironment=yes
ChangesAssociations=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "contextmenu"; Description: "Add ""Open with Wyven"" to the Explorer context menu"; GroupDescription: "Explorer:"
Name: "workspaces"; Description: "Open .code-workspace files with Wyven"; GroupDescription: "Explorer:"; Flags: unchecked
Name: "addtopath"; Description: "Add Wyven to PATH (start it with ""wyven"" in a terminal)"; GroupDescription: "Terminal:"

[Files]
Source: "..\target\release\wyven.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\docs\*"; DestDir: "{app}\docs"; Excludes: "wyven-setup*.exe"; Flags: ignoreversion recursesubdirs skipifsourcedoesntexist

[Icons]
Name: "{autoprograms}\Wyven"; Filename: "{app}\wyven.exe"
Name: "{autodesktop}\Wyven"; Filename: "{app}\wyven.exe"; Tasks: desktopicon

[Registry]
; "Open with Wyven" on files, folders and the folder background.
Root: HKCU; Subkey: "Software\Classes\*\shell\Wyven"; ValueType: string; ValueName: ""; ValueData: "Open with Wyven"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\*\shell\Wyven"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\wyven.exe"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\*\shell\Wyven\command"; ValueType: string; ValueName: ""; ValueData: """{app}\wyven.exe"" ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Wyven"; ValueType: string; ValueName: ""; ValueData: "Open with Wyven"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Wyven"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\wyven.exe"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Wyven\command"; ValueType: string; ValueName: ""; ValueData: """{app}\wyven.exe"" ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Wyven"; ValueType: string; ValueName: ""; ValueData: "Open with Wyven"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Wyven"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\wyven.exe"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Wyven\command"; ValueType: string; ValueName: ""; ValueData: """{app}\wyven.exe"" ""%V"""; Tasks: contextmenu
; .code-workspace files.
Root: HKCU; Subkey: "Software\Classes\.code-workspace\OpenWithProgids"; ValueType: string; ValueName: "Wyven.Workspace"; ValueData: ""; Flags: uninsdeletevalue; Tasks: workspaces
Root: HKCU; Subkey: "Software\Classes\Wyven.Workspace"; ValueType: string; ValueName: ""; ValueData: "Wyven Workspace"; Flags: uninsdeletekey; Tasks: workspaces
Root: HKCU; Subkey: "Software\Classes\Wyven.Workspace\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\wyven.exe"",0"; Tasks: workspaces
Root: HKCU; Subkey: "Software\Classes\Wyven.Workspace\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\wyven.exe"" ""%1"""; Tasks: workspaces
; "wyven" in a terminal.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\wyven.exe"; Description: "{cm:LaunchProgram,Wyven}"; Flags: nowait postinstall skipifsilent

[Code]
function NeedsAddPath(Dir: string): Boolean;
var
  Path: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0;
end;

{ Uninstall: take Wyven out of PATH again (settings in %APPDATA%\Wyven stay). }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Path, Dir: string;
  P: Integer;
begin
  if CurUninstallStep <> usPostUninstall then
    exit;
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
    exit;
  Dir := ExpandConstant('{app}');
  P := Pos(';' + Uppercase(Dir), Uppercase(Path));
  if P > 0 then
  begin
    Delete(Path, P, Length(Dir) + 1);
    RegWriteExpandStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path);
  end;
end;
