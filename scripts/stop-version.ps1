$ErrorActionPreference = 'Stop'
$versionRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$runtimeRoot = Join-Path $versionRoot 'runtime'
$tokenFile = Join-Path $runtimeRoot 'api-token'
$safeToStop = $true
. (Join-Path $PSScriptRoot 'version-process.ps1')
function Get-ResearchProjects {
  $items = @()
  $cursor = $null
  do {
    $url = 'http://127.0.0.1:8899/api/v2/research/projects?limit=100'
    if ($cursor) { $url += '&cursor=' + [uri]::EscapeDataString($cursor) }
    $page = Invoke-RestMethod -Uri $url -Headers $authHeaders -TimeoutSec 5
    $items += @($page.projects)
    $cursor = $page.next_cursor
  } while ($cursor)
  return $items
}
if (Test-Path -LiteralPath $tokenFile) {
  $authHeaders = @{ Authorization = ('Bearer ' + (Get-Content -LiteralPath $tokenFile -Raw).Trim()) }
  try {
    $projects = @(Get-ResearchProjects)
    foreach ($project in $projects) {
      foreach ($run in $project.runs) {
        if ($run.state -ne 'ended') {
          $authHeaders['Idempotency-Key'] = [guid]::NewGuid().ToString()
          $body = @{ type='stop_run';run_id=$run.id;target=@{kind='run';id=$run.id};apply_at='immediate';payload=@{reason='User stopped MathCat Lab 2.5.0'} } | ConvertTo-Json -Depth 5
          Invoke-RestMethod -Method Post -Uri ('http://127.0.0.1:8899/api/v2/research/projects/' + $project.id + '/commands') -Headers $authHeaders -ContentType 'application/json' -Body $body -TimeoutSec 10 | Out-Null
        }
      }
      foreach ($interaction in $project.interactions) {
        if ($interaction.state -ne 'ended') {
          $authHeaders['Idempotency-Key'] = [guid]::NewGuid().ToString()
          $interactionBody = @{ expected_revision=$interaction.revision } | ConvertTo-Json
          Invoke-RestMethod -Method Post -Uri ('http://127.0.0.1:8899/api/v2/research/projects/' + $project.id + '/interaction-executions/' + $interaction.id + '/cancel') -Headers $authHeaders -ContentType 'application/json' -Body $interactionBody -TimeoutSec 10 | Out-Null
        }
      }
    }
    $stopDeadline = [DateTime]::UtcNow.AddSeconds(45)
    do {
      $pending = @()
      foreach ($project in @(Get-ResearchProjects)) {
        $locallyClosed = Test-ProjectLocalExecutionsEnded $project
        $pending += @($project.runs | Where-Object { $_.state -ne 'ended' -or ($_.outstanding_cancellation -eq $true -and -not $locallyClosed) })
        $pending += @($project.sessions | Where-Object { $_.state -ne 'closed' })
        # An ended local call may retain unknown remote delivery/billing forever.
        # Preserve that receipt, but do not confuse it with a still-owned local process.
        $pending += @($project.usage | Where-Object { $_.state -in @('reserved','running') -or ($_.state -eq 'unknown' -and -not $_.ended_at) -or ($_.outstanding_cancellation -eq $true -and -not $locallyClosed) })
        $pending += @($project.interactions | Where-Object { $_.state -ne 'ended' -or ($_.outstanding_cancellation -eq $true -and -not $locallyClosed) })
      }
      if ($pending.Count -eq 0) { break }
      Start-Sleep -Milliseconds 500
    } while ([DateTime]::UtcNow -lt $stopDeadline)
    if ($pending.Count -gt 0) { $safeToStop = $false; Write-Warning 'Some executions are still stopping or unknown. Services were left running so their receipts remain accessible.' }
    if ($safeToStop) {
      $uncertainReceipts = @(foreach ($project in @(Get-ResearchProjects)) { $project.usage | Where-Object { $_.state -eq 'unknown' } })
      if ($uncertainReceipts.Count -gt 0) { Write-Warning 'Historical calls have unknown remote delivery/billing. Their records remain unchanged. Stopping local services does not confirm remote billing cancellation.' }
    }
  } catch {
    $researchPidFile = Join-Path $runtimeRoot 'research.pid'
    if (Test-Path -LiteralPath $researchPidFile) {
      $possibleId = 0
      if ([int]::TryParse((Get-Content -LiteralPath $researchPidFile -Raw).Trim(),[ref]$possibleId)) {
        $possibleProcess = Get-CimInstance Win32_Process -Filter "ProcessId=$possibleId" -ErrorAction SilentlyContinue
        if (Test-VersionProcess $possibleProcess 'research.pid' $versionRoot) { $safeToStop = $false }
      }
    }
    Write-Warning 'Could not confirm cancellation through the research API. No active research server will be force-killed.'
  }
} else {
  $researchPidFile = Join-Path $runtimeRoot 'research.pid'
  if (Test-Path -LiteralPath $researchPidFile) {
    $possibleId = 0
    if ([int]::TryParse((Get-Content -LiteralPath $researchPidFile -Raw).Trim(),[ref]$possibleId)) {
      $possibleProcess = Get-CimInstance Win32_Process -Filter "ProcessId=$possibleId" -ErrorAction SilentlyContinue
      if (Test-VersionProcess $possibleProcess 'research.pid' $versionRoot) {
        $safeToStop = $false
        Write-Warning 'The server token is missing. Cannot confirm cancellation; the active research service was left running.'
      }
    }
  }
}
if (-not $safeToStop) { throw 'Stop has not been confirmed. Inspect the 2.5.0 whiteboard/logs; do not assume remote billing has stopped.' }
foreach ($pidName in @('platform.pid','research.pid')) {
  $pidFile = Join-Path $runtimeRoot $pidName
  if (-not (Test-Path -LiteralPath $pidFile)) { continue }
  $targetProcessId = 0
  if (-not [int]::TryParse((Get-Content -LiteralPath $pidFile -Raw).Trim(), [ref]$targetProcessId)) { continue }
  $processInfo = Get-CimInstance Win32_Process -Filter "ProcessId=$targetProcessId" -ErrorAction SilentlyContinue
  if (-not $processInfo) { continue }
  if (-not (Test-VersionProcess $processInfo $pidName $versionRoot)) {
    Write-Warning "PID $targetProcessId does not belong to this version; left untouched."
    continue
  }
  Stop-Process -Id $targetProcessId -ErrorAction SilentlyContinue
}
Write-Output 'MathCat Lab 2.5.0 stop requested. Other versions were not stopped. No files were deleted.'
