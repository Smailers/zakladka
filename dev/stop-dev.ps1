# Developer helper: stop the --dev copy started by run-dev.ps1 (only its recorded PID, only if it runs from this folder).
$here = Split-Path -Parent $PSScriptRoot
$pidFile = Join-Path (Join-Path $env:LOCALAPPDATA 'text-bookmark-native') 'dev.pid'
if (-not (Test-Path -LiteralPath $pidFile)) { Write-Output 'no dev.pid'; exit 0 }
$old = [int](Get-Content -LiteralPath $pidFile -TotalCount 1)
$proc = Get-Process -Id $old -ErrorAction SilentlyContinue
if ($proc -and $proc.Path -and $proc.Path.StartsWith($here, [StringComparison]::OrdinalIgnoreCase)) {
    Stop-Process -Id $old -Force
    Start-Sleep -Milliseconds 500
    Write-Output "stopped $old"
} else {
    Write-Output "dev process $old is not running from this folder"
}
