# Developer helper: monitors (bounds, primary) and the cursor position, DPI-aware.
Add-Type -TypeDefinition @'
using System.Runtime.InteropServices;
public static class DpiAware { [DllImport("user32.dll")] public static extern bool SetProcessDPIAware(); }
'@
[void][DpiAware]::SetProcessDPIAware()
Add-Type -AssemblyName System.Windows.Forms
foreach ($s in [System.Windows.Forms.Screen]::AllScreens) {
    Write-Output ("{0} primary={1} bounds={2} work={3}" -f $s.DeviceName, $s.Primary, $s.Bounds, $s.WorkingArea)
}
Write-Output ("cursor={0}" -f [System.Windows.Forms.Cursor]::Position)
