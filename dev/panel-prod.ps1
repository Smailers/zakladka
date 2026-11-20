# Developer helper: list the working copy's windows, toggle its panel through the tray callback, list again.
& (Join-Path $PSScriptRoot 'windows.ps1') -Prod
& (Join-Path $PSScriptRoot 'post-tray.ps1')
Start-Sleep -Milliseconds 900
& (Join-Path $PSScriptRoot 'windows.ps1') -Prod
