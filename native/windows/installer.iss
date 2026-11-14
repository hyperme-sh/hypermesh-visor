#ifndef BundlePath
  #error BundlePath is required
#endif
#ifndef BundleVersion
  #error BundleVersion is required
#endif

[Setup]
AppId={{07DCA947-3B65-4805-B806-0F82D083690B}
AppName=Hypermesh Visor Development
AppVersion={#BundleVersion}
AppPublisher=Hyperme.sh
DefaultDirName={localappdata}\Programs\HypermeshVisorDevelopment
DefaultGroupName=Hypermesh Visor Development
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
DisableProgramGroupPage=yes
UsePreviousAppDir=yes
UsePreviousTasks=yes
CloseApplications=yes
RestartApplications=no
UninstallDisplayIcon={app}\hypermesh-native-desktop.exe
OutputBaseFilename=hypermesh-visor-{#BundleVersion}-windows-x64-unsigned-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
LicenseFile=..\..\LICENSE

[Tasks]
Name: "startup"; Description: "Start Visor when I sign in (requires configured Conductor paths)"; Flags: unchecked

[Files]
Source: "{#BundlePath}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
Type: files; Name: "{userstartup}\Hypermesh Visor Development.lnk"; Tasks: not startup

[Icons]
Name: "{group}\Hypermesh Visor"; Filename: "{app}\hypermesh-visor.exe"; WorkingDir: "{app}"
Name: "{group}\Setup and limitations"; Filename: "{app}\README.md"
Name: "{userstartup}\Hypermesh Visor Development"; Filename: "{app}\hypermesh-visor.exe"; WorkingDir: "{app}"; Tasks: startup

; User state is stored outside {app}; uninstall never removes it.
; No [Run] entry: installation does not open or authorize a desktop session.
