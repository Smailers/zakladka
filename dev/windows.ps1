# Developer helper: list visible top-level windows of the --dev copy (class, rectangle, title length only).
param([switch]$Prod)  # -Prod: the working copy started by update-install.ps1 -Exe (native.pid)
$data = Join-Path $env:LOCALAPPDATA $(if ($Prod) { 'text-bookmark' } else { 'text-bookmark-native' })
$target = [int](Get-Content -LiteralPath (Join-Path $data $(if ($Prod) { 'native.pid' } else { 'dev.pid' })) -TotalCount 1)
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class DevWindows {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr p);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern int GetWindowTextLength(IntPtr h);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    public static List<string> List(uint target) {
        var lines = new List<string>();
        IntPtr fg = GetForegroundWindow();
        EnumWindows((h, p) => {
            uint pid; GetWindowThreadProcessId(h, out pid);
            if (pid == target && IsWindowVisible(h)) {
                RECT r; GetWindowRect(h, out r);
                var c = new StringBuilder(128); GetClassName(h, c, 128);
                lines.Add(String.Format("{0} [{1},{2},{3},{4}] title_len={5}{6}", c, r.L, r.T, r.R, r.B, GetWindowTextLength(h), h == fg ? " FOREGROUND" : ""));
            }
            return true;
        }, IntPtr.Zero);
        return lines;
    }
}
'@
$found = [DevWindows]::List([uint32]$target)
Write-Output ("{0} visible windows of pid {1}" -f $found.Count, $target)
$found | ForEach-Object { Write-Output $_ }
