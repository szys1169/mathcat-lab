Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

# Build the Chinese description from Unicode code points. Windows PowerShell 5.1
# otherwise misreads UTF-8 scripts without a BOM on some machines.
$description = -join ([char[]](36873, 25321, 32, 77, 97, 116, 104, 32, 76, 97, 98, 32, 24037, 20316, 21306, 25991, 20214, 22841))

$owner = New-Object System.Windows.Forms.Form
$owner.ShowInTaskbar = $false
$owner.TopMost = $true
$owner.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::None
$owner.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
$owner.Size = New-Object System.Drawing.Size(1, 1)
$owner.Opacity = 0

$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
$dialog.Description = $description
$dialog.ShowNewFolderButton = $true
$dialog.RootFolder = [System.Environment+SpecialFolder]::MyComputer
if ($null -ne $dialog.PSObject.Properties['AutoUpgradeEnabled']) {
  $dialog.AutoUpgradeEnabled = $true
}

try {
  $owner.Show()
  $owner.Activate()
  $owner.BringToFront()
  if ($dialog.ShowDialog($owner) -eq [System.Windows.Forms.DialogResult]::OK) {
    [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
    [Console]::Write($dialog.SelectedPath)
  }
} finally {
  $dialog.Dispose()
  $owner.Close()
  $owner.Dispose()
}
