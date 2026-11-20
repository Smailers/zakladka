# Developer helper: recent antivirus events that mention Zakladka (Kaspersky event log, Windows Defender
# detections). Reads only; prints time, source and the first lines of each message.
$since = (Get-Date).AddHours(-2)
foreach ($log in @(Get-WinEvent -ListLog * -ErrorAction SilentlyContinue | Where-Object { $_.LogName -match 'Kaspersky|Defender' -and $_.RecordCount })) {
    Get-WinEvent -LogName $log.LogName -MaxEvents 200 -ErrorAction SilentlyContinue |
        Where-Object { $_.TimeCreated -ge $since -and $_.Message -match 'Zakladka' } |
        ForEach-Object { Write-Output ("{0:HH:mm:ss} [{1}] id={2}: {3}" -f $_.TimeCreated, $log.LogName, $_.Id, (($_.Message -split "`n" | Select-Object -First 6) -join ' | ')) }
}
Write-Output 'done'
