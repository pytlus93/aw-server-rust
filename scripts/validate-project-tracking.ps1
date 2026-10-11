[CmdletBinding()]
param([switch]$Offline)

$ErrorActionPreference = 'Stop'
$taskRepoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $taskRepoRoot
try {
    function Invoke-ProjectCargo {
        param([string[]]$CargoArguments)
        & cargo @CargoArguments
        if ($LASTEXITCODE -ne 0) { throw "cargo check failed: $($CargoArguments -join ' ')" }
    }
    $taskDependencyFlags = @('--locked')
    if ($Offline) { $taskDependencyFlags += '--offline' }
    & rustc --version
    if ($LASTEXITCODE -ne 0) { throw 'Rust compiler unavailable' }
    Invoke-ProjectCargo (@('fmt', '-p', 'aw-projects', '-p', 'aw-datastore', '-p', 'aw-server', '--check'))
    Invoke-ProjectCargo (@('test', '-p', 'aw-projects', '-p', 'aw-datastore') + $taskDependencyFlags)
    Invoke-ProjectCargo (@('test', '-p', 'aw-server', '--test', 'projects_api', '--test', 'api') + $taskDependencyFlags)
    Invoke-ProjectCargo (@('clippy', '-p', 'aw-projects', '-p', 'aw-datastore', '--lib', '--test', 'projects', '--test', 'project_retries') + $taskDependencyFlags + @('--', '-D', 'warnings'))
    Invoke-ProjectCargo (@('clippy', '-p', 'aw-server', '--lib', '--test', 'projects_api', '--no-deps') + $taskDependencyFlags + @('--', '-D', 'warnings'))
    Write-Output 'Project tracking local validation passed.'
} finally {
    Pop-Location
}
