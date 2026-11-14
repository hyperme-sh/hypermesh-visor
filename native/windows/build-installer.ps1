param(
    [string]$Bundle = 'dist/windows-development',
    [string]$Output = 'dist/windows-installer'
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
Set-Location $repo
$bundlePath = (Resolve-Path -LiteralPath $Bundle).Path
$outputPath = [IO.Path]::GetFullPath($Output)
if (Test-Path -LiteralPath $outputPath) { throw 'Installer output already exists' }
foreach ($file in @('hypermesh-visor.exe', 'hypermesh-native-desktop.exe', 'hypermesh_visor.dll', 'conductor.exe', 'hypermesh.exe', 'hypermesh-desktop-mcp.exe', 'SHA256SUMS', 'UNSIGNED.txt')) {
    if (!(Test-Path -LiteralPath (Join-Path $bundlePath $file) -PathType Leaf)) { throw "Incomplete bundle: $file" }
}
$listed = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach ($line in Get-Content -LiteralPath (Join-Path $bundlePath 'SHA256SUMS')) {
    $entry = [regex]::Match($line, '^([0-9a-fA-F]{64}) \*(.+)$')
    if (!$entry.Success) { throw 'Invalid bundle checksum entry' }
    $relative = $entry.Groups[2].Value.Replace('/', '\')
    if ([IO.Path]::IsPathRooted($relative) -or ($relative.Split('\') -contains '..') -or !$listed.Add($relative)) { throw 'Unsafe or duplicate bundle checksum path' }
    $file = Join-Path $bundlePath $relative
    if (!(Test-Path -LiteralPath $file -PathType Leaf) -or (Get-Item -LiteralPath $file).Attributes.HasFlag([IO.FileAttributes]::ReparsePoint)) { throw 'Bundle checksum path is not a regular file' }
    if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $entry.Groups[1].Value) { throw 'Bundle checksum mismatch' }
}
foreach ($file in Get-ChildItem -LiteralPath $bundlePath -Recurse -File) {
    $relative = [IO.Path]::GetRelativePath($bundlePath, $file.FullName)
    if ($relative -ne 'SHA256SUMS' -and !$listed.Contains($relative)) { throw 'Bundle contains an unlisted file' }
}
$manifest = Get-Content -LiteralPath Cargo.toml -Raw
$match = [regex]::Match($manifest, '(?ms)^\[package\]\s.*?^version\s*=\s*"(\d+\.\d+\.\d+)"')
if (!$match.Success) { throw 'Cannot read the recorded package version' }
$version = $match.Groups[1].Value
$compiler = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe'
if (!(Test-Path -LiteralPath $compiler)) { throw 'Inno Setup 6 is required; no host package is installed by this script' }
& $compiler "/DBundlePath=$bundlePath" "/DBundleVersion=$version" "/O$outputPath" native/windows/installer.iss
if ($LASTEXITCODE -ne 0) { throw 'Unsigned installer build failed' }
$installer = Join-Path $outputPath "hypermesh-visor-$version-windows-x64-unsigned-setup.exe"
if (!(Test-Path -LiteralPath $installer)) { throw 'Installer output missing' }
'{0} *{1}' -f (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant(), ([IO.Path]::GetFileName($installer)) |
    Set-Content -LiteralPath (Join-Path $outputPath 'SHA256SUMS') -Encoding ascii
'Unsigned development installer. Native signing remains deferred.' |
    Set-Content -LiteralPath (Join-Path $outputPath 'UNSIGNED.txt') -Encoding ascii
