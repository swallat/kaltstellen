<#
.SYNOPSIS
  Builds kaltstellen from this repository and starts it, separate from the
  real configuration.

.DESCRIPTION
  Without -Real, the configuration, the data and the Run key of start at
  sign-in point to a sandbox (%TEMP%\ks-dev, HKCU\Software\kaltstellen-dev);
  the first run copies config.example.toml there. A tray started from this
  repository's target\release is ended first (after `recover`, so nothing
  stays frozen); another installation is only reported, never ended.

.EXAMPLE
  .\scripts\dev-run.ps1            # pull, build, start the tray in the sandbox
  .\scripts\dev-run.ps1 -NoPull    # build without git pull
  .\scripts\dev-run.ps1 -Gui       # open the main window instead of the tray
  .\scripts\dev-run.ps1 -Clean     # end the sandbox tray, remove the sandbox
  .\scripts\dev-run.ps1 -Real      # use the real configuration (%APPDATA%\kaltstellen)
#>
param(
    [string]$Repo = (Split-Path -Parent $PSScriptRoot),
    [switch]$NoPull,
    [switch]$Gui,
    [switch]$Clean,
    [switch]$Real
)
$ErrorActionPreference = 'Stop'
$exe = Join-Path $Repo 'target\release\kaltstellen.exe'
$sandbox = Join-Path $env:TEMP 'ks-dev'
$runKey = 'Software\kaltstellen-dev\Run'

function Use-Sandbox {
    $env:KALTSTELLEN_CONFIG_DIR = Join-Path $sandbox 'config'
    $env:KALTSTELLEN_DATA_DIR = Join-Path $sandbox 'data'
    $env:KALTSTELLEN_RUN_KEY = $runKey
    $cfg = Join-Path $env:KALTSTELLEN_CONFIG_DIR 'config.toml'
    if (-not (Test-Path $cfg)) {
        New-Item -ItemType Directory -Force $env:KALTSTELLEN_CONFIG_DIR | Out-Null
        Copy-Item (Join-Path $Repo 'config.example.toml') $cfg
        Write-Host "Test configuration created: $cfg (put in programs you may freeze)"
    }
}

# Ends trays started from this repository; another installation is only
# reported. Returns whether no other tray is in the way.
function Stop-DevTray {
    $trays = Get-CimInstance Win32_Process -Filter "Name='kaltstellen.exe'" |
        Where-Object { $_.CommandLine -match '\btray\b' }
    foreach ($p in $trays) {
        if ($p.ExecutablePath -and ($p.ExecutablePath -ieq $exe)) {
            Write-Host "Ending the repository's tray (PID $($p.ProcessId)) after resuming everything ..."
            & $exe recover | Out-Host
            Stop-Process -Id $p.ProcessId -Force
        } else {
            Write-Warning ("Another kaltstellen tray runs: $($p.ExecutablePath) (PID $($p.ProcessId)). " +
                'Choose Neutral and then Quit in its menu, then run this script again.')
            return $false
        }
    }
    return $true
}

if (-not $Real) { Use-Sandbox }

if ($Clean) {
    $null = Stop-DevTray
    Remove-Item -Recurse -Force $sandbox -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force 'HKCU:\Software\kaltstellen-dev' -ErrorAction SilentlyContinue
    Write-Host 'Sandbox removed.'
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
        git pull --ff-only
        if ($LASTEXITCODE) { throw 'git pull failed' }
    }
    Write-Host "Commit: $(git log --oneline -1)"
    cargo build --release --locked
    if ($LASTEXITCODE) { throw 'cargo build failed' }
} finally {
    Pop-Location
}

& $exe --version
& $exe check-config
if ($LASTEXITCODE) {
    Write-Warning 'The configuration is invalid, see above.'
    exit $LASTEXITCODE
}

if ($Gui) {
    & $exe gui
} else {
    # The tray inherits the sandbox variables of this session.
    Start-Process -FilePath $exe -ArgumentList 'tray' -WindowStyle Hidden
    Write-Host "Tray started. Way out: & '$exe' recover   Clean up: .\scripts\dev-run.ps1 -Clean"
}
