# Developer helper: start a given Zakladka.exe, wait, and report whether it keeps running (exit code if not).
# With -Record the PID goes to native.pid (the working copy).
param([Parameter(Mandatory)][string]$Exe, [switch]$Record)
$p = Start-Process -FilePath $Exe -WorkingDirectory (Split-Path -Parent $Exe) -PassThru
Start-Sleep -Seconds 3
$p.Refresh()
if ($p.HasExited) { Write-Output ("exited with code {0} (0x{0:X8})" -f $p.ExitCode) }
else {
    Write-Output ("running pid {0}, private {1:N1} MB" -f $p.Id, ($p.PrivateMemorySize64 / 1MB))
    if ($Record) { Set-Content -LiteralPath (Join-Path (Join-Path $env:LOCALAPPDATA 'text-bookmark') 'native.pid') -Value $p.Id }
}
Get-Process -Name Zakladka -ErrorAction SilentlyContinue | ForEach-Object { Write-Output "process $($_.Id) $($_.Path)" }
