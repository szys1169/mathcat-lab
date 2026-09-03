param(
  [Parameter(Mandatory = $true)]
  [string]$LiteralPath
)

$ErrorActionPreference = 'Stop'
$item = Get-Item -LiteralPath $LiteralPath
$folderPath = if ($item.PSIsContainer) { $item.FullName } else { $item.DirectoryName }

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class MathLabExplorerWindow {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, IntPtr processId);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool attach);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern IntPtr SetFocus(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hWnd, int nCmdShow);
}
'@

function Show-ExplorerInForeground([IntPtr]$handle) {
  $foreground = [MathLabExplorerWindow]::GetForegroundWindow()
  $currentThread = [MathLabExplorerWindow]::GetCurrentThreadId()
  $foregroundThread = if ($foreground -ne [IntPtr]::Zero) {
    [MathLabExplorerWindow]::GetWindowThreadProcessId($foreground, [IntPtr]::Zero)
  } else { 0 }
  $targetThread = [MathLabExplorerWindow]::GetWindowThreadProcessId($handle, [IntPtr]::Zero)

  $attachedForeground = $foregroundThread -ne 0 -and $foregroundThread -ne $currentThread -and
    [MathLabExplorerWindow]::AttachThreadInput($currentThread, $foregroundThread, $true)
  $attachedTarget = $targetThread -ne 0 -and $targetThread -ne $currentThread -and
    [MathLabExplorerWindow]::AttachThreadInput($currentThread, $targetThread, $true)
  try {
    [MathLabExplorerWindow]::ShowWindowAsync($handle, 9) | Out-Null
    [MathLabExplorerWindow]::BringWindowToTop($handle) | Out-Null
    [MathLabExplorerWindow]::SetForegroundWindow($handle) | Out-Null
    [MathLabExplorerWindow]::SetFocus($handle) | Out-Null
  } finally {
    if ($attachedTarget) { [MathLabExplorerWindow]::AttachThreadInput($currentThread, $targetThread, $false) | Out-Null }
    if ($attachedForeground) { [MathLabExplorerWindow]::AttachThreadInput($currentThread, $foregroundThread, $false) | Out-Null }
  }
}

$shell = New-Object -ComObject Shell.Application

function Find-ExplorerWindow([string]$targetFolder) {
  foreach ($window in @($shell.Windows())) {
    try {
      if (-not $window.FullName.EndsWith('explorer.exe', [StringComparison]::OrdinalIgnoreCase)) { continue }
      $location = ([Uri]$window.LocationURL).LocalPath
      if ([string]::Equals((Resolve-Path -LiteralPath $location).Path.TrimEnd('\'), $targetFolder.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)) {
        return $window
      }
    } catch {}
  }
  return $null
}

$targetWindow = Find-ExplorerWindow $folderPath
if (-not $targetWindow) {
  $launchArguments = if ($item.PSIsContainer) { '"{0}"' -f $folderPath } else { '/select,"{0}"' -f $item.FullName }
  Start-Process -FilePath 'explorer.exe' -ArgumentList $launchArguments -WindowStyle Normal
  for ($attempt = 0; $attempt -lt 40 -and -not $targetWindow; $attempt++) {
    Start-Sleep -Milliseconds 50
    $targetWindow = Find-ExplorerWindow $folderPath
  }
}

if (-not $targetWindow) {
  Start-Process -FilePath 'explorer.exe' -ArgumentList ('/select,"{0}"' -f $item.FullName) -WindowStyle Normal
  exit 0
}

if (-not $item.PSIsContainer) {
  $folderItem = $targetWindow.Document.Folder.ParseName($item.Name)
  if ($folderItem) { $targetWindow.Document.SelectItem($folderItem, 29) }
}

$handle = [IntPtr]::new([int64]$targetWindow.HWND)
Show-ExplorerInForeground $handle
