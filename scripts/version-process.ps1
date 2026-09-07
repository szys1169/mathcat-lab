function Test-VersionProcess($processInfo, [string]$pidName, [string]$versionRoot) {
  if (-not $processInfo) { return $false }
  if ($pidName -eq 'research.pid') {
    $expected = Join-Path $versionRoot 'math-research-mvp\target\release\mathcat-v2.exe'
    return [string]::Equals($processInfo.ExecutablePath, $expected, [System.StringComparison]::OrdinalIgnoreCase)
  }
  if ($pidName -eq 'platform.pid' -and $processInfo.Name -eq 'node.exe' -and $processInfo.CommandLine) {
    $entry = '"' + (Join-Path $versionRoot 'math-lab-platfrom\src\server.mjs') + '"'
    return $processInfo.CommandLine.IndexOf($entry, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
  }
  return $false
}

function Test-ProjectLocalExecutionsEnded($project) {
  # This permits shutting down the local app, NOT clearing remote-delivery/billing uncertainty.
  if (@($project.sessions).Count -eq 0 -or @($project.usage).Count -eq 0) { return $false }
  if (@($project.runs | Where-Object { $_.state -ne 'ended' }).Count -gt 0) { return $false }
  if (@($project.interactions | Where-Object { $_.state -ne 'ended' }).Count -gt 0) { return $false }
  if (@($project.sessions | Where-Object { $_.state -ne 'closed' }).Count -gt 0) { return $false }
  foreach ($call in $project.usage) {
    if (-not $call.ended_at) { return $false }
    $endedAt = [DateTimeOffset]::MinValue
    if (-not [DateTimeOffset]::TryParse([string]$call.ended_at, [ref]$endedAt)) { return $false }
    if ($call.state -notin @('succeeded','failed','cancelled','unknown')) { return $false }
    if (-not $call.session_id -or -not @($project.sessions | Where-Object { $_.id -eq $call.session_id }).Count) { return $false }
  }
  foreach ($run in @($project.runs | Where-Object { $_.outstanding_cancellation -eq $true })) {
    if (-not $run.id) { return $false }
    if (-not @($project.sessions | Where-Object { $_.run_id -eq $run.id }).Count) { return $false }
    if (-not @($project.usage | Where-Object { $_.run_id -eq $run.id }).Count) { return $false }
  }
  foreach ($interaction in @($project.interactions | Where-Object { $_.outstanding_cancellation -eq $true })) {
    if (-not $interaction.id) { return $false }
    if (-not @($project.sessions | Where-Object { $_.execution_owner.kind -eq 'interaction' -and $_.execution_owner.id -eq $interaction.id }).Count) { return $false }
    if (-not @($project.usage | Where-Object { $_.execution_owner.kind -eq 'interaction' -and $_.execution_owner.id -eq $interaction.id }).Count) { return $false }
  }
  return $true
}
