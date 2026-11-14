param([string]$InstallerDirectory = 'dist/windows-installer')
$ErrorActionPreference = 'Stop'
$installers = @(Get-ChildItem -LiteralPath $InstallerDirectory -Filter '*-unsigned-setup.exe' -File)
if ($installers.Count -ne 1) { throw 'Expected one unsigned installer' }
$installer = $installers[0].FullName
$install = Join-Path $env:RUNNER_TEMP ('Visor installer ' + [guid]::NewGuid())
$startup = Join-Path ([Environment]::GetFolderPath('Startup')) 'Hypermesh Visor Development.lnk'
$programs = Join-Path ([Environment]::GetFolderPath('Programs')) 'Hypermesh Visor Development'
$data = Join-Path $env:LOCALAPPDATA 'HypermeshVisor'
$sentinel = Join-Path $data ('installer-contract-' + [guid]::NewGuid() + '.txt')
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{07DCA947-3B65-4805-B806-0F82D083690B}_is1'
if ((Test-Path -LiteralPath $startup) -or (Test-Path -LiteralPath $uninstallKey) -or (Test-Path -LiteralPath $programs)) { throw 'Installer contract requires an unused development installation' }
New-Item -ItemType Directory -Path $data -Force | Out-Null
'preserve user state' | Set-Content -LiteralPath $sentinel
function Install-Candidate([string]$Tasks) {
    $arguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', "/DIR=`"$install`"")
    $arguments += "/TASKS=$Tasks"
    $process = Start-Process -FilePath $installer -ArgumentList $arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Installer returned $($process.ExitCode)" }
}
try {
    Install-Candidate ''
    if (Test-Path -LiteralPath $startup) { throw 'Startup was enabled without selecting the task' }
    foreach ($file in @('hypermesh-visor.exe', 'hypermesh-native-desktop.exe', 'hypermesh_visor.dll', 'conductor.exe', 'hypermesh.exe', 'hypermesh-desktop-mcp.exe')) {
        if (!(Test-Path -LiteralPath (Join-Path $install $file))) { throw "Installed file missing: $file" }
    }
    $registration = Get-ItemProperty -LiteralPath $uninstallKey
    if ([IO.Path]::GetFullPath($registration.InstallLocation).TrimEnd('\') -ne [IO.Path]::GetFullPath($install).TrimEnd('\')) { throw 'Installer registered a different directory' }
    $owned = Join-Path $install 'UNSIGNED.txt'
    $expected = (Get-FileHash -LiteralPath $owned -Algorithm SHA256).Hash
    'replace this damaged owned file' | Set-Content -LiteralPath $owned
    # Exercise replacement using the recorded version; do not invent a new release.
    Install-Candidate 'startup'
    if ((Get-FileHash -LiteralPath $owned -Algorithm SHA256).Hash -ne $expected) { throw 'Reinstall did not replace the owned file' }
    if (!(Test-Path -LiteralPath $startup)) { throw 'Selected user-session startup missing' }
    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($startup)
    if ($shortcut.TargetPath -ne (Join-Path $install 'hypermesh-visor.exe')) { throw 'Startup points outside the installed launcher' }
    if ((Get-Content -LiteralPath $sentinel -Raw).Trim() -ne 'preserve user state') { throw 'Reinstall changed user data' }
    Install-Candidate ''
    if (Test-Path -LiteralPath $startup) { throw 'Deselected startup remains enabled' }
    Install-Candidate 'startup'
    if (!(Test-Path -LiteralPath $startup)) { throw 'Startup could not be re-enabled' }
    $process = Start-Process -FilePath (Join-Path $install 'unins000.exe') -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw 'Uninstall failed' }
    if ((Test-Path -LiteralPath $uninstallKey) -or (Test-Path -LiteralPath $startup) -or (Test-Path -LiteralPath (Join-Path $install 'hypermesh-visor.exe'))) { throw 'Uninstall left registered application files or startup' }
    if ((Get-Content -LiteralPath $sentinel -Raw).Trim() -ne 'preserve user state') { throw 'Uninstall changed user data' }
    Write-Output 'PASS per-user install, same-version replacement, startup selection, and data-preserving uninstall'
} finally {
    if (Test-Path -LiteralPath $sentinel) { Remove-Item -LiteralPath $sentinel }
}
