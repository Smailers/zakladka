# Developer helper: open the --dev panel through its tray callback, list windows, close it again the same way.
$here = $PSScriptRoot
& (Join-Path $here 'windows.ps1')
& (Join-Path $here 'post-tray.ps1')
Start-Sleep -Milliseconds 900
& (Join-Path $here 'windows.ps1')
Start-Sleep -Milliseconds 1200
& (Join-Path $here 'post-tray.ps1')
Start-Sleep -Milliseconds 700
& (Join-Path $here 'windows.ps1')
