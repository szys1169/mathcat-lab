$ErrorActionPreference = 'Stop'
$versionRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
. (Join-Path $versionRoot 'scripts\version-process.ps1')
$backend = Join-Path $versionRoot 'math-research-mvp\target\release\mathcat-v2.exe'
$server = Join-Path $versionRoot 'math-lab-platfrom\src\server.mjs'
if (-not (Test-VersionProcess @{ExecutablePath=$backend} 'research.pid' $versionRoot)) { throw 'Own backend rejected' }
if (Test-VersionProcess @{ExecutablePath=($backend + '.backup')} 'research.pid' $versionRoot) { throw 'Sibling backend matched' }
if (-not (Test-VersionProcess @{Name='node.exe';CommandLine=('node.exe "'+$server+'"')} 'platform.pid' $versionRoot)) { throw 'Own platform rejected' }
if (Test-VersionProcess @{Name='node.exe';CommandLine=('node.exe "'+$server+'.bak"')} 'platform.pid' $versionRoot) { throw 'Other entry matched' }
$fixture = @{runs=@(@{id='r1';state='ended';outstanding_cancellation=$true});interactions=@();sessions=@(@{id='s1';run_id='r1';state='closed'});usage=@(@{session_id='s1';run_id='r1';state='unknown';ended_at='2026-09-05T22:41:03Z'})}
if (-not (Test-ProjectLocalExecutionsEnded $fixture)) { throw 'Historical remote unknown is not a local process' }
if (-not $fixture.runs[0].outstanding_cancellation -or $fixture.usage[0].state -ne 'unknown') { throw 'Remote uncertainty must not be cleared' }
$fixture.sessions[0].state='lost'
if (Test-ProjectLocalExecutionsEnded $fixture) { throw 'Lost local session must block' }
$fixture.sessions[0].state='active'
if (Test-ProjectLocalExecutionsEnded $fixture) { throw 'Active local session must block' }
$fixture.sessions[0].state='closed'; $fixture.usage[0].ended_at=$null
if (Test-ProjectLocalExecutionsEnded $fixture) { throw 'Unknown without local end must block' }
$fixture.usage[0].ended_at='2026-09-05T22:41:03Z'; $fixture.usage[0].state='running'
if (Test-ProjectLocalExecutionsEnded $fixture) { throw 'Running call must block' }
if (Test-ProjectLocalExecutionsEnded @{runs=@(@{state='ended';outstanding_cancellation=$true});sessions=@();usage=@()}) { throw 'Missing local evidence must block' }
$fixture.usage[0].state='unknown'; $fixture.usage[0].ended_at='invalid date'
if (Test-ProjectLocalExecutionsEnded $fixture) { throw 'Invalid end timestamp must block' }
$fixture.usage[0].ended_at='2026-09-05T22:41:03Z'; $fixture.runs[0].id='unrelated-run'
if (Test-ProjectLocalExecutionsEnded $fixture) { throw 'Unrelated run evidence must block' }
$fixture.runs[0].id='r1'; $fixture.usage[0].session_id='unrelated-session'
if (Test-ProjectLocalExecutionsEnded $fixture) { throw 'Missing associated session must block' }
foreach ($file in @('start-version.ps1','stop-version.ps1','version-process.ps1')) {
  $tokens = $null; $parseErrors = $null
  [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $versionRoot ('scripts\'+$file)), [ref]$tokens, [ref]$parseErrors) | Out-Null
  if ($parseErrors) { throw "Syntax failure: $file" }
}
Write-Output '4 process ownership, 10 local/remote cancellation boundary, and 3 PowerShell syntax checks passed. No services were changed.'
