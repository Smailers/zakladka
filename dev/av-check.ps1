# Developer helper: can the exe copies be read (hash) — an antivirus that holds or quarantines a fresh
# build makes them unreadable. Also lists running Zakladka.exe processes. Reads only.
$native = Split-Path -Parent $PSScriptRoot
$paths = @(
    (Join-Path $native 'target\release\Zakladka.exe'),
    (Join-Path $native 'dist\Zakladka-2.0.0-win-x64\Zakladka.exe'),
    (Join-Path $env:LOCALAPPDATA 'Programs\Zakladka\Zakladka.exe')
)
foreach ($p in $paths) {
    if (-not (Test-Path -LiteralPath $p)) { Write-Output "MISSING  $p"; continue }
    try { $h = (Get-FileHash -LiteralPath $p -Algorithm SHA256 -ErrorAction Stop).Hash.Substring(0, 12); Write-Output "ok $h  $p" }
    catch { Write-Output "LOCKED   $p : $($_.Exception.Message)" }
}
Get-CimInstance Win32_Process -Filter "Name='Zakladka.exe'" | ForEach-Object { Write-Output "running $($_.ProcessId) $($_.ExecutablePath)" }
