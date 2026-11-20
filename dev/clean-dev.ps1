# Developer helper: stop the --dev copy and remove its test data (only the dev folder, never the working one).
& (Join-Path $PSScriptRoot 'stop-dev.ps1')
$data = Join-Path $env:LOCALAPPDATA 'text-bookmark-native'
foreach ($name in 'bookmarks.json', 'debug-render.log', 'debug-state.json', 'timing.log', 'error.log', 'stats.log') {
    $file = Join-Path $data $name
    if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file; Write-Output "removed $name" }
}
