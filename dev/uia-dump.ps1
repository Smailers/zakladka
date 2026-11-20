# Developer helper: list clickable UI Automation elements (links, list/tree/tab items, buttons) in the
# visible windows of one program, with control type, name, patterns and off-screen flag — to see how an
# app exposes its chat or tab list. Writes a UTF-8 file; reads only.
param([Parameter(Mandatory)][string]$Process, [Parameter(Mandatory)][string]$Out, [int]$Limit = 400)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$T = [System.Windows.Automation.ControlType]
$ids = @(Get-Process -Name $Process -ErrorAction SilentlyContinue | ForEach-Object Id)
$lines = New-Object System.Collections.Generic.List[string]
$kinds = $T::Hyperlink, $T::ListItem, $T::TreeItem, $T::TabItem, $T::Button, $T::MenuItem
$conditions = [System.Windows.Automation.Condition[]]@($kinds | ForEach-Object { New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, $_) })
$any = New-Object System.Windows.Automation.OrCondition($conditions)
foreach ($pid_ in $ids) {
    $byPid = New-Object System.Windows.Automation.PropertyCondition($A::ProcessIdProperty, $pid_)
    foreach ($w in $A::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $byPid)) {
        $lines.Add(("WINDOW pid={0} name={1}" -f $pid_, $w.Current.Name))
        $found = $w.FindAll([System.Windows.Automation.TreeScope]::Descendants, $any)
        $n = 0
        foreach ($e in $found) {
            if ($n -ge $Limit) { break }
            $c = $e.Current
            if (-not $c.Name) { continue }
            $p = @($e.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName -replace 'Identifiers.Pattern', '' -replace 'Pattern$', '' }) -join ','
            $lines.Add(("{0} | {1} | id={2} | off={3} | {4}" -f $c.ControlType.ProgrammaticName.Replace('ControlType.', ''), $c.Name.Substring(0, [Math]::Min(90, $c.Name.Length)), $c.AutomationId, $c.IsOffscreen, $p))
            $n++
        }
    }
}
[IO.File]::WriteAllLines($Out, $lines, (New-Object Text.UTF8Encoding $false))
Write-Output ("{0} lines -> {1}" -f $lines.Count, $Out)
