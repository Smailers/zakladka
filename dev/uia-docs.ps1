# Developer helper: names of the documents (pages, chats) one program shows right now — to see which chat
# or tab is open after a jump. Reads only.
param([Parameter(Mandatory)][string]$Process)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$document = New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::Document)
foreach ($p in @(Get-Process -Name $Process -ErrorAction SilentlyContinue)) {
    $byPid = New-Object System.Windows.Automation.PropertyCondition($A::ProcessIdProperty, $p.Id)
    foreach ($w in $A::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $byPid)) {
        foreach ($d in $w.FindAll([System.Windows.Automation.TreeScope]::Descendants, $document)) {
            if (-not $d.Current.IsOffscreen) { Write-Output ("{0:HH:mm:ss} document: {1}" -f (Get-Date), $d.Current.Name) }
        }
    }
}
