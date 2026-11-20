# Автозапуск «Закладки»: ярлык «Text Bookmark.lnk» в папке «Автозагрузка» текущего пользователя
# и ярлык «Закладка» в меню «Пуск».
#   powershell -ExecutionPolicy Bypass -File .\install_autostart.ps1
# Ярлык указывает на Zakladka.exe рядом со скриптом, а если его там нет (папка исходников) —
# на target\release\Zakladka.exe. Имя ярлыка то же, что у прежней версии на Python, поэтому её
# ярлык заменяется. Ярлык с этим именем, который указывает на другую программу, не трогается.
# Программу скрипт не запускает: она запустится при следующем входе в Windows.
$ErrorActionPreference = 'Stop'

$exe = Join-Path $PSScriptRoot 'Zakladka.exe'
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    $exe = Join-Path $PSScriptRoot 'target\release\Zakladka.exe'
}
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    Write-Output "Не найден Zakladka.exe ни рядом со скриптом, ни в target\release ($PSScriptRoot). Ничего не изменено."
    exit 1
}
$exe = (Resolve-Path -LiteralPath $exe).ProviderPath
$folder = Split-Path -Parent $exe

$startup = [Environment]::GetFolderPath([Environment+SpecialFolder]::Startup)
$link = Join-Path $startup 'Text Bookmark.lnk'
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($link)

$was = ''
if (Test-Path -LiteralPath $link) {
    $oldTarget = [string]$shortcut.TargetPath
    $oldArgs = [string]$shortcut.Arguments
    $oldName = if ($oldTarget) { [IO.Path]::GetFileName($oldTarget) } else { '' }
    $isNative = $oldName -ieq 'Zakladka.exe'
    $isPython = ($oldName -ieq 'pythonw.exe' -or $oldName -ieq 'python.exe') -and $oldArgs -like '*text_bookmark.pyw*'
    if (-not ($isNative -or $isPython)) {
        Write-Output "Ярлык $link указывает на другую программу: $oldTarget $oldArgs"
        Write-Output 'Ничего не изменено.'
        exit 1
    }
    if ($isPython) {
        $was = "заменён ярлык прежней версии на Python ($oldTarget $oldArgs)"
    } elseif ($oldTarget -ieq $exe) {
        $was = 'ярлык уже указывал сюда, обновлён'
    } else {
        $was = "ярлык перенаправлен, раньше указывал на $oldTarget"
    }
}

$shortcut.TargetPath = $exe
$shortcut.Arguments = ''
$shortcut.WorkingDirectory = $folder
$shortcut.IconLocation = "$exe,0"
$shortcut.Description = 'Закладка: отметка в тексте любого окна'
$shortcut.Save()

# Ярлык «Закладка» в меню «Пуск» — чтобы снова запустить программу после «Выход».
# Ярлык с этим именем, который указывает на другую программу, не трогается.
$programs = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
$menuLink = Join-Path $programs 'Закладка.lnk'
$menu = $shell.CreateShortcut($menuLink)
$menuTarget = [string]$menu.TargetPath
if ((Test-Path -LiteralPath $menuLink) -and $menuTarget -and [IO.Path]::GetFileName($menuTarget) -ine 'Zakladka.exe') {
    Write-Output "Ярлык $menuLink указывает на другую программу: $menuTarget. Меню «Пуск» не изменено."
} else {
    $menu.TargetPath = $exe
    $menu.Arguments = ''
    $menu.WorkingDirectory = $folder
    $menu.IconLocation = "$exe,0"
    $menu.Description = $shortcut.Description
    $menu.Save()
    Write-Output "Меню «Пуск»: $menuLink -> $exe"
}

if ($was) {
    Write-Output "Автозапуск: $link -> $exe ($was)."
} else {
    Write-Output "Создан ярлык автозапуска: $link -> $exe"
}
Write-Output 'Закладка запустится при следующем входе в Windows. Сейчас её можно запустить из меню «Пуск» («Закладка») или двойным щелчком по Zakladka.exe.'
if ($was -like '*Python*') {
    Write-Output 'Прежнюю версию сначала закройте («Выход» в её панели): вместе они не запускаются.'
}
