#Requires -Version 7.0
<#
.SYNOPSIS
    Tests which of the currently running apps survive being suspended.

.DESCRIPTION
    For every app running in your session, one at a time: suspend it with
    kaltstellen (including a memory trim), wait, resume it, and check that it
    is still alive and responding. An app that died or hangs afterwards is
    closed completely and started again with its original command line.

    Without -Run the script only lists what it would test.

    SAVE YOUR WORK FIRST. An app that does not survive the test is killed and
    restarted, and anything unsaved in it is lost.

    Results are written to local/compat-results/. The local/ folder is ignored
    by Git: results describe the software on one machine and do not belong in
    the repository. To exclude apps permanently, list their executable names,
    one per line, in local/compat-exclude.txt.

.PARAMETER Run
    Actually run the test. Without it, only list the candidates.

.PARAMETER Only
    Test only these executable names. Also allows apps installed in the
    Windows directory, which are skipped otherwise.

.PARAMETER Exclude
    Executable names to skip, in addition to local/compat-exclude.txt.

.PARAMETER SuspendSeconds
    How long each app stays suspended.

.PARAMETER SettleSeconds
    How long an app gets after resuming to become responsive again.

.PARAMETER RestartTimeoutSeconds
    How long a restarted app gets to come up.

.PARAMETER NoRestart
    Never kill and restart an app; only report that it died or hangs.

.EXAMPLE
    ./scripts/Test-SuspendCompat.ps1
    Lists the apps that would be tested.

.EXAMPLE
    ./scripts/Test-SuspendCompat.ps1 -Run -Only notepad, calc
#>
[CmdletBinding()]
param(
    [switch]$Run,
    [string[]]$Only = @(),
    [string[]]$Exclude = @(),
    [ValidateRange(1, 600)][int]$SuspendSeconds = 15,
    [ValidateRange(3, 600)][int]$SettleSeconds = 10,
    [ValidateRange(5, 600)][int]$RestartTimeoutSeconds = 30,
    [switch]$NoRestart
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path $PSScriptRoot -Parent
$localDir = Join-Path $repoRoot 'local'
$excludeFile = Join-Path $localDir 'compat-exclude.txt'
$resultDir = Join-Path $localDir 'compat-results'
$exe = Join-Path $repoRoot 'target\release\kaltstellen.exe'
$sessionId = (Get-Process -Id $PID).SessionId

# Never tested, on any machine: suspending these freezes the terminal the
# script runs in or other apps that embed them.
$alwaysExclude = @('kaltstellen', 'pwsh', 'powershell', 'cmd', 'conhost', 'openconsole',
    'windowsterminal', 'msedgewebview2')

function ConvertTo-AppName([string]$name) {
    $name.Trim().ToLowerInvariant() -replace '\.exe$', ''
}

function Get-SessionProcesses {
    Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name, ExecutablePath, CommandLine, SessionId |
        Where-Object { $_.SessionId -eq $sessionId }
}

# Names of this script's own process and everything above it (shell, terminal,
# IDE, ...). Suspending any of them would freeze the test itself.
function Get-AncestorNames {
    $byId = @{}
    foreach ($p in Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name) {
        $byId[[int]$p.ProcessId] = $p
    }
    $names = @()
    $seen = @{}
    $id = $PID
    while ($byId.ContainsKey($id) -and -not $seen.ContainsKey($id)) {
        $seen[$id] = $true
        $names += ConvertTo-AppName $byId[$id].Name
        $id = [int]$byId[$id].ParentProcessId
    }
    $names
}

function Get-Candidates {
    $excluded = @($alwaysExclude) + @(Get-AncestorNames) + @($Exclude | ForEach-Object { ConvertTo-AppName $_ })
    if (Test-Path $excludeFile) {
        $excluded += Get-Content $excludeFile |
            Where-Object { $_.Trim() -and -not $_.Trim().StartsWith('#') } |
            ForEach-Object { ConvertTo-AppName $_ }
    }
    $onlyNames = @($Only | ForEach-Object { ConvertTo-AppName $_ })
    $windowsDir = $env:windir.TrimEnd('\') + '\'

    $groups = Get-SessionProcesses |
        Where-Object { $_.ExecutablePath } |
        Group-Object { ConvertTo-AppName $_.Name }

    foreach ($group in $groups | Sort-Object Name) {
        if ($group.Name -in $excluded) { continue }
        if ($onlyNames.Count -gt 0) {
            if ($group.Name -notin $onlyNames) { continue }
        }
        elseif ($group.Group[0].ExecutablePath.StartsWith($windowsDir, [StringComparison]::OrdinalIgnoreCase)) {
            continue
        }
        $ids = @($group.Group | ForEach-Object { [int]$_.ProcessId })
        [pscustomobject]@{
            Name  = $group.Name
            Ids   = $ids
            # Processes started from outside the app; these are what a restart
            # has to bring back.
            Roots = @($group.Group | Where-Object { [int]$_.ParentProcessId -notin $ids })
        }
    }
}

function Get-AppState([int[]]$Ids) {
    $procs = @(Get-Process -Id $Ids -ErrorAction SilentlyContinue)
    $bytes = 0
    foreach ($p in $procs) { $bytes += $p.WorkingSet64 }
    $windows = @($procs | Where-Object { $_.MainWindowHandle -ne 0 })
    [pscustomobject]@{
        AliveIds  = @($procs | ForEach-Object { $_.Id })
        RamMB     = [math]::Round($bytes / 1MB, 1)
        HasWindow = $windows.Count -gt 0
        Hung      = @($windows | Where-Object { -not $_.Responding }).Count -gt 0
    }
}

function Get-Arguments([string]$CommandLine, [string]$Path) {
    if (-not $CommandLine) { return '' }
    $c = $CommandLine.Trim()
    if ($c.StartsWith('"')) {
        $end = $c.IndexOf('"', 1)
        if ($end -gt 0) { return $c.Substring($end + 1).Trim() }
    }
    if ($c.StartsWith($Path, [StringComparison]::OrdinalIgnoreCase)) {
        return $c.Substring($Path.Length).Trim()
    }
    $space = $c.IndexOf(' ')
    if ($space -gt 0) { return $c.Substring($space + 1).Trim() }
    ''
}

# Closes every process of the app and starts its root processes again.
# Returns $true if the app is running and responsive afterwards.
function Restart-App($App) {
    $name = $App.Name
    Get-SessionProcesses |
        Where-Object { (ConvertTo-AppName $_.Name) -eq $name } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Seconds 2

    $started = @{}
    foreach ($root in $App.Roots) {
        $key = "$($root.ExecutablePath)|$($root.CommandLine)"
        if ($started.ContainsKey($key)) { continue }
        $started[$key] = $true
        $params = @{
            FilePath         = $root.ExecutablePath
            WorkingDirectory = Split-Path $root.ExecutablePath -Parent
        }
        $arguments = Get-Arguments $root.CommandLine $root.ExecutablePath
        if ($arguments) { $params.ArgumentList = $arguments }
        try { Start-Process @params } catch { Write-Verbose "start failed: $_" }
    }

    $deadline = (Get-Date).AddSeconds($RestartTimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 1
        $ids = @(Get-SessionProcesses |
                Where-Object { (ConvertTo-AppName $_.Name) -eq $name } |
                ForEach-Object { [int]$_.ProcessId })
        if ($ids.Count -gt 0 -and -not (Get-AppState $ids).Hung) { return $true }
    }
    $false
}

function Test-App($App) {
    $result = [ordered]@{
        Name           = $App.Name
        Processes      = $App.Ids.Count
        HasWindow      = $false
        RamBeforeMB    = $null
        RamSuspendedMB = $null
        RamAfterMB     = $null
        Verdict        = ''
        Detail         = ''
    }

    $before = Get-AppState $App.Ids
    $result.HasWindow = $before.HasWindow
    $result.RamBeforeMB = $before.RamMB
    if ($before.Hung) {
        $result.Verdict = 'skipped'
        $result.Detail = 'was not responding before the test'
        return [pscustomobject]$result
    }

    $suspended = 0
    try {
        $output = & $exe suspend $App.Name --trim 2>&1
        $suspended = @($output | Where-Object { "$_".StartsWith('suspended') }).Count
        if ($suspended -gt 0) {
            Start-Sleep -Seconds 2
            $result.RamSuspendedMB = (Get-AppState $App.Ids).RamMB
            Start-Sleep -Seconds ([math]::Max(0, $SuspendSeconds - 2))
        }
    }
    finally {
        # Always resume, also on Ctrl+C, so nothing is left frozen.
        & $exe resume $App.Name *> $null
    }

    if ($suspended -eq 0) {
        $result.Verdict = 'no-access'
        $result.Detail = 'could not be suspended; it probably runs elevated'
        return [pscustomobject]$result
    }
    if ($suspended -lt $App.Ids.Count) {
        $result.Detail = "only $suspended of $($App.Ids.Count) processes could be suspended"
    }

    # Give the app time to recover, then judge it.
    $rootIds = @($App.Roots | ForEach-Object { [int]$_.ProcessId })
    $deadline = (Get-Date).AddSeconds($SettleSeconds)
    Start-Sleep -Seconds 3
    do {
        $after = Get-AppState $App.Ids
        $died = @($rootIds | Where-Object { $_ -notin $after.AliveIds }).Count -gt 0
        if ($died -or -not $after.Hung) { break }
        Start-Sleep -Seconds 1
    } while ((Get-Date) -lt $deadline)
    $result.RamAfterMB = $after.RamMB

    $problem = if ($died) { 'died' } elseif ($after.Hung) { 'hung' } else { $null }
    if (-not $problem) {
        $result.Verdict = 'ok'
    }
    elseif ($NoRestart) {
        $result.Verdict = $problem
    }
    elseif (Restart-App $App) {
        $result.Verdict = "$problem-restarted"
    }
    else {
        $result.Verdict = "$problem-restart-failed"
    }
    [pscustomobject]$result
}

$candidates = @(Get-Candidates)
if ($candidates.Count -eq 0) {
    Write-Host 'No apps to test.'
    return
}

if (-not $Run) {
    $candidates | ForEach-Object {
        $state = Get-AppState $_.Ids
        [pscustomobject]@{ Name = $_.Name; Processes = $_.Ids.Count; RamMB = $state.RamMB; Window = $state.HasWindow }
    } | Format-Table -AutoSize
    Write-Host "$($candidates.Count) apps would be tested. Save your work, then run again with -Run."
    Write-Host "Skip apps with -Exclude or by listing them in $excludeFile."
    return
}

Push-Location $repoRoot
try { cargo build --release --quiet } finally { Pop-Location }
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $exe)) { throw 'Building kaltstellen failed.' }

$results = @()
try {
    $index = 0
    foreach ($app in $candidates) {
        $index++
        Write-Host ("[{0}/{1}] {2} ... " -f $index, $candidates.Count, $app.Name) -NoNewline
        $r = Test-App $app
        $results += $r
        Write-Host $r.Verdict
    }
}
finally {
    if ($results.Count -gt 0) {
        New-Item -ItemType Directory -Force $resultDir | Out-Null
        $file = Join-Path $resultDir ('{0}.json' -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
        $results | ConvertTo-Json | Set-Content -Path $file -Encoding utf8
        $results | Format-Table Name, Processes, RamBeforeMB, RamSuspendedMB, RamAfterMB, Verdict, Detail -AutoSize
        Write-Host "Results saved to $file"
    }
}
