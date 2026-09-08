param([switch]$NoBrowser, [switch]$Rebuild)
$ErrorActionPreference = 'Stop'
$versionRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$backendRoot = Join-Path $versionRoot 'math-research-mvp'
$platformRoot = Join-Path $versionRoot 'math-lab-platfrom'
$runtimeRoot = Join-Path $versionRoot 'runtime'
$logRoot = Join-Path $runtimeRoot 'logs'
$dataRoot = Join-Path $versionRoot 'workspaces'
$tokenFile = Join-Path $runtimeRoot 'api-token'
$backendUrl = 'http://127.0.0.1:8900'
$platformUrl = 'http://127.0.0.1:4335'
New-Item -ItemType Directory -Force -Path $runtimeRoot,$logRoot,$dataRoot | Out-Null
function Get-Health([string]$url) {
  try { return Invoke-RestMethod -Uri $url -TimeoutSec 2 } catch { return $null }
}
function Assert-PortFree([int]$port) {
  $occupied = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue
  if ($occupied) { throw "Port $port is occupied by another process. No existing process was stopped." }
}
if (-not (Get-Command node.exe -ErrorAction SilentlyContinue)) { throw 'Node.js 22 or newer is required.' }
$nodeVersion = & node.exe --version
if ($LASTEXITCODE -ne 0 -or -not $nodeVersion) { throw 'Cannot read the installed Node.js version.' }
$nodeMajor = [int]($nodeVersion.Trim().TrimStart('v').Split('.')[0])
if ($nodeMajor -lt 22) { throw 'Node.js 22 or newer is required.' }
# Select only an already installed executor; never change the user's global model or install/update Codex here.
$codexCommand = 'codex.cmd'
if ($env:CODEX_BIN) { $codexCommand = $env:CODEX_BIN }
else {
  $codexCandidates = @()
  if ($env:LOCALAPPDATA) {
    $appBinRoot = Join-Path $env:LOCALAPPDATA 'OpenAI\Codex\bin'
    if (Test-Path -LiteralPath $appBinRoot) {
      $codexCandidates += @(Get-ChildItem -LiteralPath $appBinRoot -Directory | ForEach-Object { Join-Path $_.FullName 'codex.exe' })
    }
  }
  if ($env:APPDATA) {
    $codexCandidates += Join-Path $env:APPDATA 'npm\node_modules\@openai\codex\node_modules\@openai\codex-win32-x64\vendor\x86_64-pc-windows-msvc\bin\codex.exe'
  }
  $installedExecutors = @(foreach ($candidate in $codexCandidates) {
    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
      try {
        $candidateVersion = (& $candidate --version 2>$null | Out-String).Trim()
        if ($LASTEXITCODE -eq 0 -and $candidateVersion -match '(\d+\.\d+\.\d+)') {
          [pscustomobject]@{ Path=$candidate; Version=[version]$Matches[1] }
        }
      } catch { }
    }
  })
  $selectedExecutor = $installedExecutors | Sort-Object Version -Descending | Select-Object -First 1
  if ($selectedExecutor) { $codexCommand = $selectedExecutor.Path }
}
# Both the research service and web task executor inherit this process-local selection.
$env:CODEX_BIN = $codexCommand
if (-not (Test-Path -LiteralPath (Join-Path $platformRoot 'node_modules\katex'))) {
  Push-Location $platformRoot
  try { & npm.cmd ci --ignore-scripts; if ($LASTEXITCODE -ne 0) { throw 'Failed to install platform dependencies.' } }
  finally { Pop-Location }
}
$backendHealth = Get-Health ($backendUrl + '/health')
if ($backendHealth -and $backendHealth.version -ne '2.5.1') { throw 'Research port belongs to another version. No old version was changed.' }
if (-not $backendHealth) {
  Assert-PortFree 8900
  $backendBin = Join-Path $backendRoot 'target\release\mathcat-v2.exe'
  if ($Rebuild -or -not (Test-Path -LiteralPath $backendBin)) {
    if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) { throw 'Build requires the Rust toolchain. Run cargo build --release --bin mathcat-v2.' }
    Push-Location $backendRoot
    try { & cargo.exe build --release --bin mathcat-v2; if ($LASTEXITCODE -ne 0) { throw 'Research backend build failed.' } }
    finally { Pop-Location }
  }
  $backendArguments = @('--bind','127.0.0.1:8900','--database',('"' + (Join-Path $runtimeRoot 'state_v2.sqlite') + '"'),'--data-root',('"' + $dataRoot + '"'),'--token-file',('"' + $tokenFile + '"'),'--codex-command',('"' + $codexCommand + '"'))
  $backendProcess = Start-Process -FilePath $backendBin -ArgumentList $backendArguments -WorkingDirectory $backendRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $logRoot 'research.stdout.log') -RedirectStandardError (Join-Path $logRoot 'research.stderr.log') -PassThru
  $backendProcess.Id | Out-File -LiteralPath (Join-Path $runtimeRoot 'research.pid') -Encoding ascii
  for ($attempt=0; $attempt -lt 60; $attempt++) {
    $backendProcess.Refresh()
    if ($backendProcess.HasExited) { throw "Research process exited during startup. Logs: $logRoot" }
    if (Get-Health ($backendUrl + '/health')) { break }
    Start-Sleep -Milliseconds 300
  }
  $startedBackendHealth = Get-Health ($backendUrl + '/health')
  if (-not $startedBackendHealth) { throw "Research startup failed. Logs: $logRoot" }
  if ($startedBackendHealth.version -ne '2.5.1') { throw 'New research process reports the wrong version. Rebuild this version before using it.' }
}
$platformHealth = Get-Health ($platformUrl + '/api/health')
if ($platformHealth -and $platformHealth.version -ne '2.5.1') { throw 'Web port belongs to another version. No old version was changed.' }
if (-not $platformHealth) {
  Assert-PortFree 4335
  $env:MATH_LAB_PORT = '4335'
  $env:MATH_LAB_RUNTIME_ROOT = Join-Path $platformRoot 'runtime-data'
  $env:MATHCAT_V2_API_URL = $backendUrl
  $env:MATHCAT_V2_TOKEN_FILE = $tokenFile
  $platformProcess = Start-Process -FilePath 'node.exe' -ArgumentList @(('"' + (Join-Path $platformRoot 'src\server.mjs') + '"')) -WorkingDirectory $platformRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $logRoot 'platform.stdout.log') -RedirectStandardError (Join-Path $logRoot 'platform.stderr.log') -PassThru
  $platformProcess.Id | Out-File -LiteralPath (Join-Path $runtimeRoot 'platform.pid') -Encoding ascii
  for ($attempt=0; $attempt -lt 60; $attempt++) {
    $platformProcess.Refresh()
    if ($platformProcess.HasExited) { throw "Platform process exited during startup. Logs: $logRoot" }
    if (Get-Health ($platformUrl + '/api/health')) { break }
    Start-Sleep -Milliseconds 300
  }
  $startedPlatformHealth = Get-Health ($platformUrl + '/api/health')
  if (-not $startedPlatformHealth) { throw "Platform startup failed. Logs: $logRoot" }
  if ($startedPlatformHealth.version -ne '2.5.1') { throw 'New web process reports the wrong version. Check this version before using it.' }
}
Write-Output "MathCat Lab 2.5.1: $platformUrl"
if (-not $NoBrowser) { Start-Process -FilePath $platformUrl }
