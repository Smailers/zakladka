# Developer helper: show what Windows reads from an exe or shortcut — the file's version info and its icon
# at 16, 24, 32, 48, 64 and 256 px (PrivateExtractIcons), drawn side by side on a light and a dark strip.
param([string]$Path = (Join-Path (Split-Path -Parent $PSScriptRoot) 'target\release\Zakladka.exe'),
      [string]$Out = (Join-Path $env:TEMP 'zakladka-icon-check.png'))
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class IconCheck {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern uint PrivateExtractIcons(string file, int index, int cx, int cy, IntPtr[] icons, int[] ids, uint count, uint flags);
    [DllImport("user32.dll")] public static extern bool DestroyIcon(IntPtr h);
}
'@
$item = Get-Item -LiteralPath $Path
if ($item.Extension -ieq '.exe') {
    $v = $item.VersionInfo
    Write-Output ("FileDescription={0} | ProductName={1} | FileVersion={2} | Company={3}" -f $v.FileDescription, $v.ProductName, $v.FileVersion, $v.CompanyName)
}
$sizes = 16, 24, 32, 48, 64, 256
$bmp = New-Object System.Drawing.Bitmap 760, 560
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::White)
$g.FillRectangle([System.Drawing.Brushes]::Black, 0, 280, 760, 280)
$x = 8
foreach ($s in $sizes) {
    $h = New-Object IntPtr[] 1; $id = New-Object int[] 1
    $n = [IconCheck]::PrivateExtractIcons($Path, 0, $s, $s, $h, $id, 1, 0)
    if ($n -ge 1 -and $h[0] -ne [IntPtr]::Zero) {
        $icon = [System.Drawing.Icon]::FromHandle($h[0])
        $g.DrawIcon($icon, $x, 8); $g.DrawIcon($icon, $x, 288)
        [void][IconCheck]::DestroyIcon($h[0])
        Write-Output "icon $s px: ok"
    } else { Write-Output "icon $s px: none" }
    $x += $s + 12
}
$g.Dispose(); $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
Write-Output "picture: $Out"
