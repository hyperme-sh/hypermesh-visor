$ErrorActionPreference = 'Stop'
if (!$env:CI -or !$env:RUNNER_TEMP) { throw 'Native desktop fixture contracts run only in CI' }
$output = Join-Path $env:RUNNER_TEMP ('visor-desktop-contract-' + [guid]::NewGuid())
& dotnet publish native/windows/contracts/DesktopContracts.csproj -c Release -r win-x64 --self-contained true -p:RestoreLockedMode=true -o $output
if ($LASTEXITCODE -ne 0) { throw 'Desktop fixture build failed' }
$stdout = Join-Path $output 'stdout.txt'
$stderr = Join-Path $output 'stderr.txt'
$process = $null
try {
    $process = Start-Process -FilePath (Join-Path $output 'visor-desktop-contract.exe') -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    if (!$process.WaitForExit(30000)) { $process.Kill($true); throw 'Desktop fixture timed out' }
    $process.WaitForExit()
    Get-Content -LiteralPath $stdout
    Get-Content -LiteralPath $stderr
    if ($process.ExitCode -ne 0) { throw 'Native desktop fixture contracts failed' }
} finally {
    if ($process -and !$process.HasExited) { $process.Kill($true); $process.WaitForExit() }
    if ($process) { $process.Dispose() }
    Remove-Item -LiteralPath $output -Recurse -Force
}
