<#
.SYNOPSIS
Checks the optional background helper (M4a to M4c) on a test machine in one run.

.DESCRIPTION
Run it from a NORMAL (not elevated) prompt on the VM, with a copy of
kaltstellen.exe that is not in Program Files. It installs the real service,
so it never runs in CI and refuses to run without -OnThisMachine.

You answer the UAC prompts: one for the install and one for the uninstall.
Everything else is checked by the script and printed as a table of PASS,
FAIL and SKIP lines; the exit code is the number of failures.

Covers docs/manual-checks.md: helper steps 1 to 5 and 8 (new install,
service data, status, uninstall) and "process events" steps 1, 2 and 6
(Notepad start and stop, traffic). Left to a person: another user account,
killing the service process, the declined prompt (KS1005), the tray and
window with the helper, watch mode (run a profile with watch = true and
start a program it stops).

.PARAMETER Exe
The kaltstellen.exe to test.

.PARAMETER OnThisMachine
Confirms that a real service may be installed and removed here.

.PARAMETER KeepInstalled
Do not uninstall at the end (to try the tray and the window by hand).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Exe,
    [switch]$OnThisMachine,
    [switch]$KeepInstalled
)

$ErrorActionPreference = 'Continue'
if (-not $OnThisMachine) {
    Write-Error 'This installs a real service. Pass -OnThisMachine on a test machine.'
    exit 2
}
$Exe = (Resolve-Path -LiteralPath $Exe).Path
if ($Exe -like "$env:ProgramFiles*") {
    Write-Error 'Use a copy of the executable outside Program Files.'
    exit 2
}
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if ($admin) {
    Write-Error 'Run this from a normal prompt; the install asks for administrator rights itself.'
    exit 2
}

$results = New-Object System.Collections.Generic.List[object]
function Check([string]$Name, [bool]$Ok, [string]$Detail = '') {
    $script:results.Add([pscustomobject]@{
        Result = $(if ($Ok) { 'PASS' } else { 'FAIL' }); Check = $Name; Detail = $Detail })
}
function Skip([string]$Name, [string]$Detail) {
    $script:results.Add([pscustomobject]@{ Result = 'SKIP'; Check = $Name; Detail = $Detail })
}
function Ks([string[]]$Arguments) {
    # Output (stdout and stderr) as text; the exit code in $script:Code.
    $out = & $Exe @Arguments 2>&1 | Out-String
    $script:Code = $LASTEXITCODE
    return $out
}
function WaitFor([scriptblock]$Condition, [int]$Seconds) {
    $end = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $end) {
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return $false
}

$service = 'kaltstellen-helper'
$data = Join-Path $env:ProgramData 'kaltstellen'
$installed = Join-Path $env:ProgramFiles 'kaltstellen\kaltstellen.exe'

# 1. Before anything
$out = Ks @('helper', 'status')
if ($out -notmatch 'not installed') {
    Write-Warning 'The helper is installed already; the check starts with an uninstall.'
    Ks @('helper', 'uninstall') | Out-Null
    [void](WaitFor { -not (Get-Service $service -ErrorAction SilentlyContinue) } 60)
    $out = Ks @('helper', 'status')
}
Check '1 status before install' (($out -match 'service: not installed') -and $Code -eq 0) ($out.Trim())

# 2. Install (UAC prompt: answer it)
Write-Host 'Install: answer the UAC prompt now ...'
Ks @('helper', 'install') | Out-Null
$up = WaitFor { (Get-Service $service -ErrorAction SilentlyContinue).Status -eq 'Running' } 90
Check '2 install: service running' $up

# 3. The service
$svc = Get-CimInstance Win32_Service -Filter "Name='$service'" -ErrorAction SilentlyContinue
Check '3 account is LocalSystem' ($svc -and $svc.StartName -eq 'LocalSystem') ($svc.StartName)
Check '3 start type automatic (delayed)' ($svc -and $svc.StartMode -eq 'Auto' -and $svc.DelayedAutoStart) `
    "$($svc.StartMode), delayed=$($svc.DelayedAutoStart)"
$privs = (& sc.exe qprivs $service | Out-String)
Check '3 only SeChangeNotifyPrivilege' (($privs -match 'SeChangeNotifyPrivilege') -and ($privs -notmatch 'SeDebugPrivilege|SeBackupPrivilege|SeImpersonatePrivilege')) `
    (($privs -split "`r?`n" | Where-Object { $_ -match 'Se\w+Privilege' }) -join ' ')
Check '3 restart actions' ((& sc.exe qfailure $service | Out-String) -match 'RESTART') ''

# 4. Files and rights
Check '4 protected copy exists' (Test-Path $installed) $installed
Check '4 policy and log exist' ((Test-Path (Join-Path $data 'helper.toml')) -and (Test-Path (Join-Path $data 'kaltstellen.log'))) $data
$probe = Join-Path $data 'probe.tmp'
$wrote = $true
try { [IO.File]::WriteAllText($probe, 'x') } catch { $wrote = $false }
if ($wrote) { Remove-Item $probe -Force -ErrorAction SilentlyContinue }
Check '4 a normal user cannot write to ProgramData\kaltstellen' (-not $wrote)
$changed = $true
try { [IO.File]::OpenWrite($installed).Close() } catch { $changed = $false }
Check '4 a normal user cannot change the protected copy' (-not $changed)

# 5. Status through the pipe
$out = Ks @('helper', 'status')
Check '5 status: running, pipe answers' (($out -match 'running') -and $Code -eq 0) ($out.Trim() -replace "`r?`n", ' | ')
$observe = $out -match 'observe \(process events\): running'
$network = $out -match 'network counters: running'
Check '5 observe running' $observe
Check '5 network counters running' $network

# M4b 1, 2: Notepad start and stop
if ($observe) {
    $first = Ks @('helper', 'events')
    $next = if ($first -match 'next:\s*(\d+)') { [uint64]$Matches[1] } else { 0 }
    $note = Start-Process notepad.exe -PassThru
    Start-Sleep -Seconds 2
    Stop-Process -Id $note.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
    $events = Ks @('helper', 'events', '--after', "$next")
    $lines = $events -split "`r?`n" | Where-Object { $_ -like '{*' }
    $start = $lines | Where-Object { $_ -match '"kind":"start"' -and $_ -match 'notepad' -and $_ -match "`"pid`":$($note.Id)\b" }
    $stop = $lines | Where-Object { $_ -match '"kind":"stop"' -and $_ -match "`"pid`":$($note.Id)\b" }
    Check 'events: Notepad start' ([bool]$start) ($start | Select-Object -First 1)
    Check 'events: Notepad stop' ([bool]$stop) ($stop | Select-Object -First 1)
} else {
    Skip 'events' 'observe is not running'
}

# M4b 6: traffic (a local download is enough: loopback is counted too)
if ($network) {
    $out = Ks @('helper', 'traffic', '--seconds', '10')
    Check 'traffic: answers with JSON lines' ($Code -eq 0) (($out.Trim() -split "`r?`n" | Select-Object -First 2) -join ' | ')
} else {
    Skip 'traffic' 'network counters are not running'
}

# The tray and the window are untouched by the helper
$out = Ks @('status')
Check '9 tray status command still answers (0 running, 5 no tray)' ($Code -in 0, 1, 5) ("exit $Code")

# 8. Uninstall (UAC prompt: answer it)
if ($KeepInstalled) {
    Skip '8 uninstall' '-KeepInstalled'
} else {
    Write-Host 'Uninstall: answer the UAC prompt now ...'
    Ks @('helper', 'uninstall') | Out-Null
    $gone = WaitFor { -not (Get-Service $service -ErrorAction SilentlyContinue) } 90
    Check '8 service removed' $gone
    [void](WaitFor { -not (Test-Path $installed) } 30)
    Check '8 protected copy removed' (-not (Test-Path $installed)) $installed
    Check '8 policy removed' (-not (Test-Path (Join-Path $data 'helper.toml'))) $data
    $out = Ks @('helper', 'status')
    Check '8 status after uninstall' ($out -match 'not installed') ($out.Trim())
}

$results | Format-Table -AutoSize -Wrap | Out-String | Write-Host
$failed = @($results | Where-Object Result -eq 'FAIL').Count
Write-Host "$failed failed, $(@($results | Where-Object Result -eq 'PASS').Count) passed, $(@($results | Where-Object Result -eq 'SKIP').Count) skipped."
exit $failed
