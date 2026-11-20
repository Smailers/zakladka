# Developer helper: poll until a file becomes readable (an antivirus finished with it) or the time is up.
param([Parameter(Mandatory)][string]$Path, [int]$Seconds = 150)
$deadline = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $deadline) {
    if (-not (Test-Path -LiteralPath $Path)) { Write-Output ("{0:HH:mm:ss} GONE (removed or quarantined)" -f (Get-Date)); exit 2 }
    try { $h = (Get-FileHash -LiteralPath $Path -ErrorAction Stop).Hash.Substring(0, 12); Write-Output ("{0:HH:mm:ss} readable {1}" -f (Get-Date), $h); exit 0 }
    catch { Start-Sleep -Seconds 10 }
}
Write-Output ("{0:HH:mm:ss} still locked" -f (Get-Date)); exit 1
