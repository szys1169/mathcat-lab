function Invoke-InstalledUpdate([string]$Action, [bool]$NoBrowser = $false, [bool]$Rebuild = $false) {
  $currentRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
  $pointerFile = Join-Path $currentRoot 'runtime\updates\installed.json'
  if (-not (Test-Path -LiteralPath $pointerFile)) { return $false }
  $pointer = Get-Content -LiteralPath $pointerFile -Raw | ConvertFrom-Json
  $target = [IO.Path]::GetFullPath($pointer.destination)
  if ((Split-Path $target) -ne (Split-Path $currentRoot) -or (Split-Path $target -Leaf) -ne ('MathCat-Lab-' + $pointer.version)) { throw 'Invalid installed update path.' }
  $localVersion = (Get-Content -LiteralPath (Join-Path $currentRoot 'VERSION') -Raw).Trim()
  if ([version]$pointer.version -le [version]$localVersion) { throw 'Installed update must be newer.' }
  $arguments = @()
  if ($Action -eq 'start') { if ($NoBrowser) { $arguments += '-NoBrowser' }; if ($Rebuild) { $arguments += '-Rebuild' } }
  & (Join-Path $target ('scripts\' + $Action + '-version.ps1')) @arguments
  if (-not $?) { throw 'Updated version launcher failed.' }
  return $true
}
