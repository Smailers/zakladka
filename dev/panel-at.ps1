# Developer helper: put the cursor at a fixed point (physical pixels), then toggle the working copy's panel
# through its tray callback, so the panel always opens at the same place.
param([int]$X = 700, [int]$Y = -900)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class PanelAt {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
}
'@
[void][PanelAt]::SetProcessDPIAware()
[void][PanelAt]::SetCursorPos($X, $Y)
& (Join-Path $PSScriptRoot 'post-tray.ps1')
