$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$url = 'http://127.0.0.1:4321/'
$mathCatUrl = 'http://127.0.0.1:8787/'
$mathCatRoot = [System.IO.Path]::GetFullPath((Join-Path $root '..\math-research-mvp'))
$logDir = Join-Path $root 'runtime-data\logs'
function Test-Platform { try { return (Invoke-WebRequest -UseBasicParsing -Uri ($url + 'api/health') -TimeoutSec 2).StatusCode -eq 200 } catch { return $false } }
function Test-MathCat { try { return (Invoke-WebRequest -UseBasicParsing -Uri ($mathCatUrl + 'health') -TimeoutSec 2).StatusCode -eq 200 } catch { return $false } }
function Get-LocalSetting([string]$name, [string]$fallback) {
  $file = Join-Path $root '.env.local'
  if (-not (Test-Path -LiteralPath $file)) { return $fallback }
  $line = Get-Content -LiteralPath $file | Where-Object { $_ -match ('^' + [regex]::Escape($name) + '=') } | Select-Object -Last 1
  if (-not $line) { return $fallback }
  return ($line.Substring($line.IndexOf('=') + 1)).Trim().Trim('"').Trim("'")
}
if (-not (Get-Command node -ErrorAction SilentlyContinue)) { throw 'Node.js is not available in PATH.' }
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
if (-not (Test-MathCat)) {
  $mathCatBin = Join-Path $mathCatRoot 'target\release\math-research-agent.exe'
  if (-not (Test-Path -LiteralPath $mathCatBin)) { throw "MathCat release binary is missing: $mathCatBin. Run cargo build --release -p math-research-agent first." }
  New-Item -ItemType Directory -Force -Path (Join-Path $mathCatRoot 'runtime\platform-integration\artifacts'), (Join-Path $mathCatRoot 'runtime\platform-integration\projects') | Out-Null
  $env:MRA_DATABASE_URL = 'sqlite://runtime/platform-integration/research.db'
  $env:MRA_ARTIFACT_ROOT = 'runtime/platform-integration/artifacts'
  $env:MRA_RUNTIME_ROOT = 'runtime/platform-integration/projects'
  $env:MRA_OUTPUT_ROOT = 'runtime/platform-integration/output'
  $mathCatProcess = Start-Process -FilePath $mathCatBin -ArgumentList @('serve','--bind','127.0.0.1:8787') -WorkingDirectory $mathCatRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $logDir 'mathcat-stdout.log') -RedirectStandardError (Join-Path $logDir 'mathcat-stderr.log') -PassThru
  Set-Content -LiteralPath (Join-Path $logDir 'mathcat-process.pid') -Value $mathCatProcess.Id -Encoding ascii
  for ($i=0; $i -lt 100 -and -not (Test-MathCat); $i++) { Start-Sleep -Milliseconds 300 }
}
if (-not (Test-MathCat)) { throw "MathCat failed to start. See $logDir\mathcat-stderr.log" }
$actorId = Get-LocalSetting 'MATHCAT_ACTOR_ID' 'mathcat-lab'
$actorToken = Get-LocalSetting 'MATHCAT_API_TOKEN' ''
if ([string]::IsNullOrWhiteSpace($actorToken)) {
  $tokenBytes = New-Object byte[] 32
  [Security.Cryptography.RandomNumberGenerator]::Fill($tokenBytes)
  $actorToken = [Convert]::ToHexString($tokenBytes).ToLowerInvariant()
  Add-Content -LiteralPath (Join-Path $root '.env.local') -Value "MATHCAT_API_TOKEN=$actorToken" -Encoding utf8
}
$bootstrapBody = @{ actor_id=$actorId; display_name='MathCat Lab'; token=$actorToken } | ConvertTo-Json
try { Invoke-RestMethod -Method Post -Uri ($mathCatUrl + 'api/v1/actors/bootstrap') -ContentType 'application/json' -Body $bootstrapBody | Out-Null } catch { if ($_.ErrorDetails.Message -notmatch 'bootstrap is closed|actor already exists') { throw } }
if (-not (Test-Platform)) {
  $process = Start-Process -FilePath 'node.exe' -ArgumentList @('src/server.mjs') -WorkingDirectory $root -WindowStyle Hidden -RedirectStandardOutput (Join-Path $logDir 'stdout.log') -RedirectStandardError (Join-Path $logDir 'stderr.log') -PassThru
  Set-Content -LiteralPath (Join-Path $logDir 'process.pid') -Value $process.Id -Encoding ascii
  for ($i=0; $i -lt 50 -and -not (Test-Platform); $i++) { Start-Sleep -Milliseconds 300 }
}
if (-not (Test-Platform)) { throw "Platform failed to start. See $logDir\stderr.log" }
$platformHealth = Invoke-RestMethod -Uri ($url + 'api/health') -TimeoutSec 3
if ($platformHealth.mathcat.status -ne 'ok') { throw 'Platform started, but its MathCat connection is unavailable.' }
Start-Process $url
