# Developer helper: (re)start the --dev copy of Zakladka next to the installed one.
# Stops only the previous dev process whose PID this script recorded and whose path is this folder.
param([switch]$DebugBuild)
$here = Split-Path -Parent $PSScriptRoot
$data = Join-Path $env:LOCALAPPDATA 'text-bookmark-native'
New-Item -ItemType Directory -Force -Path $data | Out-Null
$config = Join-Path $data 'config.json'
if (-not (Test-Path -LiteralPath $config)) {
    '{ "debug": true }' | Set-Content -LiteralPath $config -Encoding utf8
    Write-Output 'config created (debug on)'
}
$pidFile = Join-Path $data 'dev.pid'
if (Test-Path -LiteralPath $pidFile) {
    $old = [int](Get-Content -LiteralPath $pidFile -TotalCount 1)
    $proc = Get-Process -Id $old -ErrorAction SilentlyContinue
    if ($proc -and $proc.Path -and $proc.Path.StartsWith($here, [StringComparison]::OrdinalIgnoreCase)) {
        Stop-Process -Id $old -Force
        Write-Output "stopped $old"
        Start-Sleep -Milliseconds 400
    }
}
$kind = if ($DebugBuild) { 'debug' } else { 'release' }
$exe = Join-Path $here "target\$kind\Zakladka.exe"
$started = Start-Process -FilePath $exe -ArgumentList '--dev' -WorkingDirectory $here -PassThru
$started.Id | Set-Content -LiteralPath $pidFile
Write-Output "started $($started.Id) ($kind)"
