# Developer helper: remove the working copy's Start-menu shortcut (only if it points to the installed
# Zakladka.exe) and create it again with install_autostart.ps1, so the Start menu re-reads its icon.
$install = Join-Path $env:LOCALAPPDATA 'Programs\Zakladka'
$exe = Join-Path $install 'Zakladka.exe'
$shell = New-Object -ComObject WScript.Shell
$programs = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
Get-ChildItem -LiteralPath $programs -Filter '*.lnk' | Where-Object { $shell.CreateShortcut($_.FullName).TargetPath -ieq $exe } | ForEach-Object {
    Remove-Item -LiteralPath $_.FullName
    Write-Output "removed $($_.Name)"
}
Start-Sleep -Seconds 3
& (Join-Path $install 'install_autostart.ps1')
