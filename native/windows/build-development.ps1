param([string]$Output = "dist/windows-development")
$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
Set-Location $repo
if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') { throw 'Windows development bundle currently requires x64' }
$out = [System.IO.Path]::GetFullPath($Output)
if (Test-Path -LiteralPath $out) { throw 'Output already exists' }
& cargo rustc --locked --lib --crate-type cdylib
if ($LASTEXITCODE -ne 0) { throw 'Native engine build failed' }
& cargo build --locked -p hypermesh-visor-client --bins
if ($LASTEXITCODE -ne 0) { throw 'Inverse MCP build failed' }
& dotnet publish native/windows/app/HypermeshVisor.csproj -c Release -r win-x64 --self-contained true -p:RestoreLockedMode=true -o $out
if ($LASTEXITCODE -ne 0) { throw 'Native companion build failed' }
Copy-Item -LiteralPath target/debug/hypermesh_visor.dll -Destination $out
Copy-Item -LiteralPath target/debug/hypermesh-acp.exe -Destination $out
Copy-Item -LiteralPath target/debug/hypermesh-desktop-mcp-client.exe -Destination (Join-Path $out 'hypermesh-desktop-mcp.exe')
Push-Location third_party/hypermesh-cli
try {
    & go build -mod=readonly -buildvcs=false -trimpath -o (Join-Path $out 'hypermesh.exe') ./cmd/hypermesh
    if ($LASTEXITCODE -ne 0) { throw 'Bundled CLI build failed' }
} finally { Pop-Location }
Push-Location native/windows/launcher
try {
    & go build -mod=readonly -buildvcs=false -trimpath -ldflags '-H windowsgui' -o (Join-Path $out 'hypermesh-visor.exe') .
    if ($LASTEXITCODE -ne 0) { throw 'Windows launcher build failed' }
} finally { Pop-Location }
$revision = '71ec23aa1646b4533e42b2d8296871e65d5e93b9'
$digest = '97737999df403c5a4952cd726ffacea95a6a0b3c4b11901b9c6824b681e1073f'
$source = Join-Path ([IO.Path]::GetTempPath()) ('visor-conductor-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $source | Out-Null
try {
    $archive = Join-Path $source 'source.tar.gz'
    Invoke-WebRequest -Uri "https://codeload.github.com/hyperme-sh/hypermesh-conductor/tar.gz/$revision" -OutFile $archive
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $digest) { throw 'Conductor source checksum mismatch' }
    & tar -xzf $archive --strip-components=1 -C $source
    if ($LASTEXITCODE -ne 0) { throw 'Conductor source extraction failed' }
    $previousCgo = $env:CGO_ENABLED
    Push-Location $source
    try {
        $env:CGO_ENABLED = '0'
        & go build -mod=readonly -buildvcs=false -trimpath -o (Join-Path $out 'conductor.exe') ./cmd/conductor
        if ($LASTEXITCODE -ne 0) { throw 'Conductor build failed' }
    } finally {
        Pop-Location
        if ($null -eq $previousCgo) { Remove-Item Env:CGO_ENABLED -ErrorAction SilentlyContinue } else { $env:CGO_ENABLED = $previousCgo }
    }
} finally { Remove-Item -LiteralPath $source -Recurse -Force }
$revision | Set-Content (Join-Path $out 'conductor-source-revision.txt') -Encoding ascii
& git rev-parse HEAD | Set-Content (Join-Path $out 'visor-source-revision.txt') -Encoding ascii
'Unsigned development bundle. Native signing remains deferred.' | Set-Content (Join-Path $out 'UNSIGNED.txt') -Encoding ascii
Copy-Item -LiteralPath native/windows/README.md -Destination (Join-Path $out 'README.md')
if (!$env:HYPERMESH_CRASH_SDK_PATH) { throw 'HYPERMESH_CRASH_SDK_PATH must select the matching CI SDK artifact' }
$visorRevision = (& git rev-parse HEAD).Trim()
& python3 native/crash/stage-runtime.py $env:HYPERMESH_CRASH_SDK_PATH (Join-Path $out 'crash-sdk') --platform windows --revision $visorRevision
if ($LASTEXITCODE -ne 0) { throw 'Native crash SDK staging failed' }
$files = Get-ChildItem -LiteralPath $out -Recurse -File | Sort-Object FullName
$manifest = foreach ($file in $files) {
    $relative = [IO.Path]::GetRelativePath($out, $file.FullName).Replace('\', '/')
    '{0} *{1}' -f (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $relative
}
$manifest | Set-Content (Join-Path $out 'SHA256SUMS') -Encoding ascii
