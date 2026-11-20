# Developer helper: copy the scripts and README into the working install (%LOCALAPPDATA%\Programs\Zakladka)
# and re-run its install_autostart.ps1. With -Exe also replaces Zakladka.exe: stops the working copy
# (only its native.pid, only if it runs from the install folder), copies the release exe, starts it.
param([switch]$Exe)
$ErrorActionPreference = 'Stop'
$native = Split-Path -Parent $PSScriptRoot
$install = Join-Path $env:LOCALAPPDATA 'Programs\Zakladka'
$pidFile = Join-Path (Join-Path $env:LOCALAPPDATA 'text-bookmark') 'native.pid'
Copy-Item -LiteralPath (Join-Path $native 'install_autostart.ps1'), (Join-Path $native 'uninstall_autostart.ps1'), (Join-Path $native 'README.md') -Destination $install -Force
if ($Exe) {
    $old = [int](Get-Content -LiteralPath $pidFile -TotalCount 1)
    $proc = Get-Process -Id $old -ErrorAction SilentlyContinue
    if ($proc -and $proc.Path -and $proc.Path.StartsWith($install, [StringComparison]::OrdinalIgnoreCase)) {
        Stop-Process -Id $old -Force
        Wait-Process -Id $old -Timeout 10 -ErrorAction SilentlyContinue
        Write-Output "stopped $old"
    }
    Copy-Item -LiteralPath (Join-Path $native 'target\release\Zakladka.exe') -Destination $install -Force
    # Свежий exe антивирус может несколько секунд не давать запустить («Отказано в доступе»).
    $new = $null
    for ($try = 1; $try -le 10 -and -not $new; $try++) {
        try { $new = Start-Process -FilePath (Join-Path $install 'Zakladka.exe') -WorkingDirectory $install -PassThru }
        catch { Write-Output "start attempt $try failed: $($_.Exception.Message)"; Start-Sleep -Seconds 2 }
    }
    if (-not $new) { Write-Output 'NOT STARTED'; exit 1 }
    Set-Content -LiteralPath $pidFile -Value $new.Id
    Write-Output "started $($new.Id)"
}
& (Join-Path $install 'install_autostart.ps1')
