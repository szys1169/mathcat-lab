param([Parameter(Mandatory=$true)][string]$Uri,[Parameter(Mandatory=$true)][string]$OutputFile)
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$parsed = [Uri]$Uri
if ($parsed.Scheme -ne 'https' -or $parsed.Host -notin @('api.github.com','github.com')) { throw 'Unsupported update source.' }
Invoke-WebRequest -Uri $Uri -OutFile $OutputFile -UseBasicParsing -UserAgent 'MathCat-Lab-Updater' -Headers @{Accept='application/vnd.github+json'} -TimeoutSec 180
if ((Get-Item -LiteralPath $OutputFile).Length -gt 300000000) { throw 'Update response exceeds the size limit.' }
