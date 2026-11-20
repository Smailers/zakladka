# Выпуск «Закладки»: сборка release, проверка библиотек, архив и контрольные суммы в dist\.
#   powershell -ExecutionPolicy Bypass -File .\dist.ps1
# 1. build.ps1 release; код выхода не 0 — выпуск остановлен.
# 2. Таблица импорта Zakladka.exe (обычная и отложенная): допустимы только системные DLL, которые
#    есть в любой Windows 10/11. Любая другая (например libgcc_s_seh-1.dll) останавливает выпуск.
# 3. dist\Zakladka-<версия>-win-x64\ (Zakladka.exe, README.md, LICENSE, install_autostart.ps1,
#    uninstall_autostart.ps1) -> dist\Zakladka-<версия>-win-x64.zip; рядом dist\SHA256SUMS.txt
#    с суммами архива и Zakladka.exe. Версия — из раздела [package] файла Cargo.toml.
# Повторный запуск пересоздаёт эти файлы; остальное в dist\ не трогается.
# Каталог сборки — как у build.ps1: $env:CARGO_TARGET_DIR, если задан, иначе target\.
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot

$allowedDlls = @(
    'kernel32', 'user32', 'gdi32', 'advapi32', 'ole32', 'oleaut32', 'combase', 'uiautomationcore',
    'ntdll', 'msvcrt', 'bcrypt', 'bcryptprimitives', 'shell32', 'comdlg32', 'dwmapi', 'shcore'
)
$packageFiles = @('README.md', 'LICENSE', 'install_autostart.ps1', 'uninstall_autostart.ps1')

# version из [package] в Cargo.toml.
function Get-PackageVersion([string]$path) {
    $section = ''
    foreach ($line in [IO.File]::ReadAllLines($path)) {
        if ($line -match '^\s*\[([^\]]*)\]') {
            $section = $Matches[1].Trim()
        } elseif ($section -eq 'package' -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
            return $Matches[1]
        }
    }
    return $null
}

# Строка ASCII до нулевого байта.
function Read-CString([byte[]]$bytes, [int]$at) {
    $end = [Array]::IndexOf($bytes, [byte]0, $at)
    if ($end -lt 0) { throw "Строка без завершающего нуля по смещению $at" }
    return [Text.Encoding]::ASCII.GetString($bytes, $at, $end - $at)
}

# Смещение в файле для RVA.
function Convert-RvaToOffset($sections, [uint64]$rva) {
    foreach ($s in $sections) {
        $size = [Math]::Max([uint64]$s.VirtualSize, [uint64]$s.RawSize)
        if ($rva -ge $s.VirtualAddress -and $rva -lt ([uint64]$s.VirtualAddress + $size)) {
            return [int]($rva - $s.VirtualAddress + $s.RawPointer)
        }
    }
    throw ("RVA 0x{0:X} вне секций файла" -f $rva)
}

# Имена DLL из таблицы импорта и таблицы отложенного импорта exe x64 (PE32+).
function Get-PeImports([string]$path) {
    $b = [IO.File]::ReadAllBytes($path)
    if ($b.Length -lt 0x40 -or $b[0] -ne 0x4D -or $b[1] -ne 0x5A) { throw "$path — не исполняемый файл (нет MZ)" }
    $pe = [BitConverter]::ToInt32($b, 0x3C)
    if ($pe -lt 0 -or $pe + 24 -gt $b.Length -or [BitConverter]::ToUInt32($b, $pe) -ne 0x4550) {
        throw "$path — нет заголовка PE"
    }
    $machine = [BitConverter]::ToUInt16($b, $pe + 4)
    $count = [BitConverter]::ToUInt16($b, $pe + 6)
    $optionalSize = [BitConverter]::ToUInt16($b, $pe + 20)
    $optional = $pe + 24
    if ($machine -ne 0x8664 -or [BitConverter]::ToUInt16($b, $optional) -ne 0x20B) {
        throw "$path — не x64 (PE32+)"
    }
    $imageBase = [BitConverter]::ToUInt64($b, $optional + 24)
    $directoryCount = [BitConverter]::ToUInt32($b, $optional + 108)
    $directories = $optional + 112
    $sectionTable = $optional + $optionalSize
    $sections = @(for ($i = 0; $i -lt $count; $i++) {
        $at = $sectionTable + 40 * $i
        [pscustomobject]@{
            VirtualSize    = [BitConverter]::ToUInt32($b, $at + 8)
            VirtualAddress = [BitConverter]::ToUInt32($b, $at + 12)
            RawSize        = [BitConverter]::ToUInt32($b, $at + 16)
            RawPointer     = [BitConverter]::ToUInt32($b, $at + 20)
        }
    })
    $names = New-Object System.Collections.Generic.List[string]

    # IMAGE_IMPORT_DESCRIPTOR, 20 байт; имя DLL — RVA по смещению 12; конец — нулевая запись.
    $importRva = [BitConverter]::ToUInt32($b, $directories + 8)
    if ($directoryCount -gt 1 -and $importRva -ne 0) {
        $at = Convert-RvaToOffset $sections $importRva
        while ($true) {
            $nameRva = [BitConverter]::ToUInt32($b, $at + 12)
            $firstThunk = [BitConverter]::ToUInt32($b, $at + 16)
            if ($nameRva -eq 0 -and $firstThunk -eq 0) { break }
            $names.Add((Read-CString $b (Convert-RvaToOffset $sections $nameRva)))
            $at += 20
        }
    }

    # Отложенный импорт: 32 байта на запись; имя по смещению 4; бит 0 атрибутов — RVA, иначе VA.
    if ($directoryCount -gt 13) {
        $delayRva = [BitConverter]::ToUInt32($b, $directories + 13 * 8)
        if ($delayRva -ne 0) {
            $at = Convert-RvaToOffset $sections $delayRva
            while ($true) {
                $attributes = [BitConverter]::ToUInt32($b, $at)
                $name = [uint64][BitConverter]::ToUInt32($b, $at + 4)
                if ($name -eq 0) { break }
                if (($attributes -band 1) -eq 0) { $name = $name - $imageBase }
                $names.Add((Read-CString $b (Convert-RvaToOffset $sections $name)))
                $at += 32
            }
        }
    }
    return @($names | ForEach-Object { $_.ToLowerInvariant() } | Sort-Object -Unique)
}

# ---- версия и пути

$version = Get-PackageVersion (Join-Path $root 'Cargo.toml')
if (-not $version -or $version -notmatch '^[0-9A-Za-z.+-]+$') {
    Write-Output "Не удалось прочитать version из [package] в Cargo.toml (получено: '$version'). Выпуск остановлен."
    exit 1
}
$targetDir = Join-Path $root 'target'
if ($env:CARGO_TARGET_DIR) {
    $targetDir = $env:CARGO_TARGET_DIR
    if (-not [IO.Path]::IsPathRooted($targetDir)) { $targetDir = Join-Path $root $targetDir }
}
$exe = Join-Path $targetDir 'release\Zakladka.exe'
$name = "Zakladka-$version-win-x64"
$dist = Join-Path $root 'dist'
$stage = Join-Path $dist $name
$zip = Join-Path $dist "$name.zip"
$sums = Join-Path $dist 'SHA256SUMS.txt'

foreach ($file in $packageFiles) {
    if (-not (Test-Path -LiteralPath (Join-Path $root $file) -PathType Leaf)) {
        Write-Output "Нет файла $file в $root. Выпуск остановлен."
        exit 1
    }
}
$cargoExe = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path -LiteralPath $cargoExe -PathType Leaf) -and -not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Output 'Не найден cargo (ни в %USERPROFILE%\.cargo\bin, ни в PATH). Выпуск остановлен.'
    exit 1
}

# ---- 1. сборка

Write-Output "== Zakladka ${version}: build.ps1 release"
# Строки cargo в stderr не должны становиться прерывающими ошибками PowerShell 5.1.
$ErrorActionPreference = 'Continue'
& (Join-Path $root 'build.ps1') release
$buildCode = $LASTEXITCODE
$ErrorActionPreference = 'Stop'
if ($buildCode -ne 0) {
    Write-Output "build.ps1 release завершился с кодом $buildCode. Выпуск остановлен."
    exit 1
}
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    Write-Output "После сборки нет $exe. Выпуск остановлен."
    exit 1
}

# ---- 2. библиотеки

$imports = Get-PeImports $exe
$foreign = @($imports | Where-Object {
    $base = $_ -replace '\.dll$', ''
    -not (($allowedDlls -contains $base) -or ($base -like 'api-ms-win-*'))
})
Write-Output ("== Импорт Zakladka.exe: " + ($imports -join ', '))
if ($foreign.Count -gt 0) {
    Write-Output ("Несистемные библиотеки: " + ($foreign -join ', ') + ". Их нужно вшить статически. Выпуск остановлен.")
    exit 1
}

# ---- 3. dist\

New-Item -ItemType Directory -Force -Path $dist | Out-Null
foreach ($old in @($stage, $zip, $sums)) {
    if (Test-Path -LiteralPath $old) { Remove-Item -LiteralPath $old -Recurse -Force }
}
New-Item -ItemType Directory -Path $stage | Out-Null
Copy-Item -LiteralPath $exe -Destination $stage
foreach ($file in $packageFiles) {
    Copy-Item -LiteralPath (Join-Path $root $file) -Destination $stage
}

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::CreateFromDirectory($stage, $zip, [IO.Compression.CompressionLevel]::Optimal, $false)
$archive = [IO.Compression.ZipFile]::OpenRead($zip)
try {
    $entries = @($archive.Entries | ForEach-Object { $_.FullName } | Sort-Object)
} finally {
    $archive.Dispose()
}
$expected = @(@('Zakladka.exe') + $packageFiles | Sort-Object)
if (($entries -join '|') -ne ($expected -join '|')) {
    Write-Output ("В архиве не те файлы: " + ($entries -join ', ') + ". Выпуск остановлен.")
    exit 1
}

$stagedExe = Join-Path $stage 'Zakladka.exe'
$lines = foreach ($file in @($zip, $stagedExe)) {
    '{0}  {1}' -f (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant(), (Split-Path -Leaf $file)
}
[IO.File]::WriteAllText($sums, (($lines -join "`n") + "`n"), (New-Object Text.UTF8Encoding($false)))

Write-Output "== Готово: $dist"
foreach ($file in @($zip, $stagedExe)) {
    Write-Output ("  {0}  {1:N0} байт" -f (Split-Path -Leaf $file), (Get-Item -LiteralPath $file).Length)
}
Write-Output ("  в архиве: " + ($entries -join ', '))
Write-Output '  SHA256SUMS.txt:'
$lines | ForEach-Object { Write-Output "    $_" }
exit 0
