# Developer helper: memory and CPU of the running --dev copy (PID from run-dev.ps1).
$data = Join-Path $env:LOCALAPPDATA 'text-bookmark-native'
$id = [int](Get-Content -LiteralPath (Join-Path $data 'dev.pid') -TotalCount 1)
$proc = Get-Process -Id $id -ErrorAction SilentlyContinue
if (-not $proc) { Write-Output "dev process $id is not running"; exit 1 }
$uptime = (Get-Date) - $proc.StartTime
$line = '{0} pid={1} private={2:N1}MB ws={3:N1}MB cpu={4:N2}s uptime={5:N0}s cpu%core={6:N3} handles={7} gdi/user via threads={8}' -f `
    (Get-Date -Format 'HH:mm:ss'), $id, ($proc.PrivateMemorySize64 / 1MB), ($proc.WorkingSet64 / 1MB), $proc.CPU, `
    $uptime.TotalSeconds, (100 * $proc.CPU / [Math]::Max(1, $uptime.TotalSeconds)), $proc.HandleCount, $proc.Threads.Count
Write-Output $line
Add-Content -LiteralPath (Join-Path $data 'stats.log') -Value $line
