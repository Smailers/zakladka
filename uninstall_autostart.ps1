# Убрать автозапуск «Закладки»: удалить ярлык «Text Bookmark.lnk» из папки «Автозагрузка»
# текущего пользователя и ярлык «Закладка» из меню «Пуск», только если они указывают на Zakladka.exe (из любой папки).
#   powershell -ExecutionPolicy Bypass -File .\uninstall_autostart.ps1
# Ярлык прежней версии на Python и ярлык другой программы с тем же именем не трогаются.
# Саму программу скрипт не закрывает: для этого «Выход» в панели.
$ErrorActionPreference = 'Stop'

# Ярлык «Закладка» в меню «Пуск» (его создаёт install_autostart.ps1) — тоже только если он на Zakladka.exe.
$programs = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
$menuLink = Join-Path $programs 'Закладка.lnk'
if (Test-Path -LiteralPath $menuLink -PathType Leaf) {
    $menuTarget = [string](New-Object -ComObject WScript.Shell).CreateShortcut($menuLink).TargetPath
    if ($menuTarget -and [IO.Path]::GetFileName($menuTarget) -ieq 'Zakladka.exe') {
        Remove-Item -LiteralPath $menuLink
        Write-Output "Удалён ярлык меню «Пуск»: $menuLink."
    }
}

$startup = [Environment]::GetFolderPath([Environment+SpecialFolder]::Startup)
$link = Join-Path $startup 'Text Bookmark.lnk'
if (-not (Test-Path -LiteralPath $link -PathType Leaf)) {
    Write-Output "Ярлыка автозапуска нет: $link. Ничего не изменено."
    exit 0
}

$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($link)
$target = [string]$shortcut.TargetPath
$name = if ($target) { [IO.Path]::GetFileName($target) } else { '' }
if ($name -ine 'Zakladka.exe') {
    Write-Output "Ярлык $link указывает не на Zakladka.exe, а на: $target $($shortcut.Arguments)"
    Write-Output 'Например, это ярлык прежней версии на Python. Ничего не удалено.'
    exit 1
}

Remove-Item -LiteralPath $link
Write-Output "Удалён ярлык автозапуска: $link (указывал на $target)."
