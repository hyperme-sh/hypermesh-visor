$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'Disposable Windows CI only' }
$name = 'HypermeshCrashRegistration'
if (Get-Service -Name $name -ErrorAction SilentlyContinue) { throw 'Fixture requires absent service' }
$exe = Join-Path $env:RUNNER_TEMP 'visor-crash-service-contract.exe'
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
try {
  New-Service -Name $name -BinaryPathName ('"' + $exe + '" ' + $sid) -StartupType Manual | Out-Null
  Start-Service -Name $name
  $env:HYPERMESH_CRASH_SERVICE_CONTRACT = '1'
  cargo test --locked -p hypermesh-local-transport windows_crash::tests::windows_crash_registration_service_contract -- --ignored --exact
  if ($LASTEXITCODE -ne 0) { throw 'LocalSystem transport contract failed' }
} finally {
  Stop-Service -Name $name -ErrorAction SilentlyContinue
  & sc.exe delete $name | Out-Null
  Remove-Item Env:HYPERMESH_CRASH_SERVICE_CONTRACT -ErrorAction SilentlyContinue
}
