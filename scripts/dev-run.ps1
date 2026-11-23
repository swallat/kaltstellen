<#
.SYNOPSIS
  Builds kaltstellen from this repository and starts it, separate from the
  real configuration.

.DESCRIPTION
  Without -Real, the configuration, the data and the Run key of start at
  sign-in point to a sandbox (%TEMP%\ks-dev, HKCU\Software\kaltstellen-dev);
  the first run copies config.example.toml there. -Real uses the real ones.
  The variables are set for this script and the programs it starts only;
  the calling shell keeps its own.

  Before building, the tray of the same mode is ended with `kaltstellen quit`
  (it resumes what it froze). A kaltstellen.exe of this repository that
  still runs afterwards (a tray of the other mode, a main window) is only
  reported, never ended; another installation is only reported, and blocks
  only with -Real (it would hold the same single-instance mutex).

.EXAMPLE
  .\scripts\dev-run.ps1            # pull, build, start the tray in the sandbox
  .\scripts\dev-run.ps1 -NoPull    # build without git pull
  .\scripts\dev-run.ps1 -Gui       # open the main window instead of the tray
  .\scripts\dev-run.ps1 -Recover   # the way out: resume everything (sandbox)
  .\scripts\dev-run.ps1 -Clean     # end the sandbox tray, remove the sandbox
  .\scripts\dev-run.ps1 -Real      # use the real configuration (%APPDATA%\kaltstellen)
#>
param(
    [string]$Repo = (Split-Path -Parent $PSScriptRoot),
    [switch]$NoPull,
    [switch]$Gui,
    [switch]$Recover,
    [switch]$Clean,
    [switch]$Real
)
$ErrorActionPreference = 'Stop'
$Repo = (Resolve-Path $Repo).ProviderPath
$exe = Join-Path $Repo 'target\release\kaltstellen.exe'
$sandbox = Join-Path $env:TEMP 'ks-dev'
$names = 'KALTSTELLEN_CONFIG_DIR', 'KALTSTELLEN_DATA_DIR', 'KALTSTELLEN_RUN_KEY'

function Set-Mode {
    foreach ($name in $names) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
    if ($Real) { return }
    $env:KALTSTELLEN_CONFIG_DIR = Join-Path $sandbox 'config'
    $env:KALTSTELLEN_DATA_DIR = Join-Path $sandbox 'data'
    $env:KALTSTELLEN_RUN_KEY = 'Software\kaltstellen-dev\Run'
    $cfg = Join-Path $env:KALTSTELLEN_CONFIG_DIR 'config.toml'
    if (-not (Test-Path $cfg)) {
        New-Item -ItemType Directory -Force $env:KALTSTELLEN_CONFIG_DIR | Out-Null
        Copy-Item (Join-Path $Repo 'config.example.toml') $cfg
        Write-Host "Test configuration created: $cfg (put in programs you may freeze)"
    }
}

# A native program; its stderr is not an error of this script.
function Invoke-Native([string]$file, [string[]]$arguments) {
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $file @arguments | Out-Host } finally { $ErrorActionPreference = $previous }
    return $LASTEXITCODE
}

function Get-Kaltstellen {
    Get-CimInstance Win32_Process -Filter "Name='kaltstellen.exe'"
}

# Ends the tray of this mode through its pipe and waits until no
# kaltstellen.exe of this repository runs. Returns whether the way is free.
function Stop-DevTray {
    if (Test-Path $exe) {
        $code = Invoke-Native $exe @('quit')
        # A build from before `quit` existed refuses it; that only matters
        # if a kaltstellen.exe of this repository runs (checked below).
        if ($code -ne 0) { Write-Warning "kaltstellen quit ended with exit code $code." }
    }
    $ours = $null
    for ($i = 0; $i -lt 50; $i++) {
        $ours = @(Get-Kaltstellen | Where-Object { $_.ExecutablePath -and ($_.ExecutablePath -ieq $exe) })
        if ($ours.Count -eq 0) { break }
        Start-Sleep -Milliseconds 200
    }
    foreach ($p in $ours) {
        Write-Warning ("kaltstellen.exe of this repository still runs (PID $($p.ProcessId)): " +
            'a tray of the other mode (run again with or without -Real, and -Clean or Quit in its menu), ' +
            'a build from before `kaltstellen quit` existed (Neutral, then Quit in its menu) ' +
            'or a main window (close it).')
    }
    $others = @(Get-Kaltstellen | Where-Object { -not ($_.ExecutablePath -and ($_.ExecutablePath -ieq $exe)) })
    foreach ($p in $others) {
        $path = if ($p.ExecutablePath) { $p.ExecutablePath } else { 'path not readable (elevated?)' }
        Write-Warning "Another kaltstellen.exe runs: $path (PID $($p.ProcessId))."
    }
    if ($Real -and $others.Count -gt 0) {
        Write-Warning 'With -Real it would hold the tray of this session: choose Neutral and then Quit in its menu first.'
        return $false
    }
    return ($ours.Count -eq 0)
}

$saved = @{}
foreach ($name in $names) { $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
try {
    Set-Mode

    if ($Recover) {
        if (-not (Test-Path $exe)) { throw "$exe does not exist yet." }
        exit (Invoke-Native $exe @('recover'))
    }

    if ($Clean) {
        $null = Stop-DevTray
        if (-not $Real) {
            Remove-Item -Recurse -Force $sandbox -ErrorAction SilentlyContinue
            Remove-Item -Recurse -Force 'HKCU:\Software\kaltstellen-dev' -ErrorAction SilentlyContinue
            if (Test-Path $sandbox) {
                Write-Warning "$sandbox could not be removed completely (a program still uses it)."
            } else {
                Write-Host 'Sandbox removed.'
            }
        }
        return
    }

    foreach ($tool in 'git', 'cargo') {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "$tool is not installed (see README, Build)."
        }
    }

    # The build cannot replace an executable that runs.
    if (-not (Stop-DevTray)) { exit 1 }

    Push-Location $Repo
    try {
        if (-not $NoPull) {
            # Without an upstream git writes to stderr; under Stop, Windows
            # PowerShell 5.1 would turn that into an error of this script.
            & {
                $ErrorActionPreference = 'Continue'
                git rev-parse --abbrev-ref '@{u}' 2>$null | Out-Null
            }
            if ($LASTEXITCODE -ne 0) {
                Write-Warning 'This branch has no upstream; building without git pull.'
            } elseif ((Invoke-Native 'git' @('pull', '--ff-only')) -ne 0) {
                throw 'git pull failed'
            }
        }
        Write-Host "Commit: $(git log --oneline -1)"
        if ((Invoke-Native 'cargo' @('build', '--release', '--locked')) -ne 0) { throw 'cargo build failed' }
    } finally {
        Pop-Location
    }

    $null = Invoke-Native $exe @('--version')
    if ((Invoke-Native $exe @('check-config')) -ne 0) {
        Write-Warning 'The configuration is invalid, see above.'
        exit 2
    }

    $mode = if ($Real) { ' -Real' } else { '' }
    if ($Gui) {
        $null = Invoke-Native $exe @('gui')
    } else {
        # Copies this script's environment, so the tray keeps the mode.
        Start-Process -FilePath $exe -ArgumentList 'tray' -WindowStyle Hidden
        Write-Host "Tray started. Way out: .\scripts\dev-run.ps1 -Recover$mode   End and clean up: .\scripts\dev-run.ps1 -Clean$mode"
    }
} finally {
    foreach ($name in $names) {
        # Removed, not set to "" (pwsh 7 passes $null on as an empty value).
        if ($null -eq $saved[$name]) {
            Remove-Item "Env:$name" -ErrorAction SilentlyContinue
        } else {
            [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process')
        }
    }
}
