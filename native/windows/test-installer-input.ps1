$ErrorActionPreference = 'Stop'
if (!$env:CI -or !$env:RUNNER_TEMP) { throw 'Installer input contracts require CI' }
$bundle = (Resolve-Path (Join-Path $PSScriptRoot '../../dist/windows-development')).Path
$fixture = Join-Path $env:RUNNER_TEMP ('visor-installer-input-' + [guid]::NewGuid())
$link = Join-Path $bundle 'unlisted-directory-link'
if (Test-Path -LiteralPath $link) { throw 'Contract link already exists' }
New-Item -ItemType Directory -Path $fixture | Out-Null
try {
    'must never be packaged' | Set-Content -LiteralPath (Join-Path $fixture 'outside.txt')
    New-Item -ItemType Junction -Path $link -Target $fixture | Out-Null
    $rejected = $false
    try {
        & "$PSScriptRoot/build-installer.ps1" -Bundle $bundle -Output (Join-Path $fixture 'output')
    } catch {
        if ($_.Exception.Message -ne 'Bundle contains a reparse point') { throw }
        $rejected = $true
    }
    if (!$rejected) { throw 'Installer accepted a directory junction' }
    if (Test-Path -LiteralPath (Join-Path $fixture 'output')) { throw 'Rejected bundle produced installer output' }
} finally {
    if (Test-Path -LiteralPath $link) { [IO.Directory]::Delete($link) }
    Remove-Item -LiteralPath $fixture -Recurse -Force
}
