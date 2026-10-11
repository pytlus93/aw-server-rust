[CmdletBinding()]
param([switch]$Release, [switch]$Offline)
$ErrorActionPreference = 'Stop'
$taskRepoRoot = Split-Path -Parent $PSScriptRoot
$taskWebui = Join-Path (Split-Path -Parent $taskRepoRoot) 'aw-webui/dist'
if (!(Test-Path (Join-Path $taskWebui 'index.html'))) {
    throw 'Build the sibling aw-webui first with its scripts/validate-project-tracking.ps1.'
}
$taskOriginalAssets = $env:AW_WEBUI_DIR
Push-Location $taskRepoRoot
try {
    $env:AW_WEBUI_DIR = (Resolve-Path $taskWebui).Path
    $taskBuildArguments = @('build', '-p', 'aw-server', '--bin', 'aw-server', '--locked')
    if ($Release) { $taskBuildArguments += '--release' }
    if ($Offline) { $taskBuildArguments += '--offline' }
    & cargo @taskBuildArguments
    if ($LASTEXITCODE -ne 0) { throw 'Project server build failed.' }
    Write-Output 'Built the AW Rust server with embedded project-management UI.'
} finally {
    $env:AW_WEBUI_DIR = $taskOriginalAssets
    Pop-Location
}
