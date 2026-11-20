# Developer helper: fingerprint of the clipboard (format count + SHA-256 of the text), never the content itself.
Add-Type -AssemblyName System.Windows.Forms
$data = [System.Windows.Forms.Clipboard]::GetDataObject()
$formats = if ($data) { $data.GetFormats().Count } else { 0 }
$text = [System.Windows.Forms.Clipboard]::GetText()
$sha = [System.Security.Cryptography.SHA256]::Create()
$hash = [BitConverter]::ToString($sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($text))).Replace('-', '').Substring(0, 16)
Write-Output ("{0} clipboard formats={1} text_len={2} sha={3}" -f (Get-Date -Format 'HH:mm:ss'), $formats, $text.Length, $hash)
