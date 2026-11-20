# Developer helper: imitate a left click on the --dev tray icon (posts the tray callback to the owner window).
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class DevTray {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
}
'@
$owner = [DevTray]::FindWindow('ZakladkaOwner', [NullString]::Value)
if ($owner -eq [IntPtr]::Zero) { Write-Output 'owner window not found'; exit 1 }
$ok = [DevTray]::PostMessage($owner, 0x8001, [IntPtr]::Zero, [IntPtr]0x0202)
Write-Output "posted=$ok"
