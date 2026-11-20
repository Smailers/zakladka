# Сборка «Закладки».
#   .\build.ps1            -> cargo check --all-targets
#   .\build.ps1 check      -> cargo check --all-targets
#   .\build.ps1 test       -> cargo test
#   .\build.ps1 build      -> cargo build
#   .\build.ps1 release    -> cargo build --release
# Остальные аргументы передаются cargo как есть, например: .\build.ps1 test --test core
#                                                        .\build.ps1 test --no-run
# Каталог сборки: $env:CARGO_TARGET_DIR, если задан (у каждого агента свой, например target-3),
# иначе target\. Код выхода — код выхода cargo.
param(
    [ValidateSet('check', 'test', 'build', 'release')]
    [string]$Action = 'check',
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)

$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (-not (($env:PATH -split ';') -contains $cargoBin)) {
    $env:PATH = "$cargoBin;$env:PATH"
}

switch ($Action) {
    'check'   { $cargoArgs = @('check', '--all-targets') }
    'test'    { $cargoArgs = @('test') }
    'build'   { $cargoArgs = @('build') }
    'release' { $cargoArgs = @('build', '--release') }
}
if ($Rest) { $cargoArgs += $Rest }

if ($env:CARGO_TARGET_DIR) {
    Write-Host "CARGO_TARGET_DIR = $env:CARGO_TARGET_DIR"
}

Push-Location -LiteralPath $PSScriptRoot
try {
    & cargo @cargoArgs
    $code = $LASTEXITCODE
}
finally {
    Pop-Location
}
exit $code
