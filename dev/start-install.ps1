# Developer helper: start the working copy (%LOCALAPPDATA%\Programs\Zakladka\Zakladka.exe) if it is not
# running, retrying while an antivirus still holds a fresh exe, and record its PID in native.pid.
$install = Join-Path $env:LOCALAPPDATA 'Programs\Zakladka'
$exe = Join-Path $install 'Zakladka.exe'
$pidFile = Join-Path (Join-Path $env:LOCALAPPDATA 'text-bookmark') 'native.pid'
$running = @(Get-CimInstance Win32_Process -Filter "Name='Zakladka.exe'" | Where-Object { $_.ExecutablePath -ieq $exe })
if ($running.Count) { Write-Output "already running: $($running[0].ProcessId)"; Set-Content -LiteralPath $pidFile -Value $running[0].ProcessId; exit 0 }
$new = $null
for ($try = 1; $try -le 15 -and -not $new; $try++) {
    try { $new = Start-Process -FilePath $exe -WorkingDirectory $install -PassThru }
    catch { Write-Output "start attempt $try failed: $($_.Exception.Message)"; Start-Sleep -Seconds 2 }
}
if (-not $new) { Write-Output 'NOT STARTED'; exit 1 }
Set-Content -LiteralPath $pidFile -Value $new.Id
Start-Sleep -Seconds 2
Write-Output ("started {0}, running={1}" -f $new.Id, [bool](Get-Process -Id $new.Id -ErrorAction SilentlyContinue))
