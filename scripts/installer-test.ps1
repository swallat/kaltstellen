<#
.SYNOPSIS
Installs, upgrades and uninstalls a kaltstellen setup and checks each step.

.DESCRIPTION
For CI and for a test machine. kaltstellen's own files stay in a sandbox
(KALTSTELLEN_CONFIG_DIR, KALTSTELLEN_DATA_DIR, KALTSTELLEN_RUN_KEY below a
temporary folder), but Windows' own places of a per-user installation are
real: the uninstall entry in HKCU and the shortcut in the Start menu. That
is why it runs only in CI ($env:CI) or with -OnThisMachine, and refuses
when kaltstellen is installed here already. Everything it creates is
removed again, also when a check fails.

Steps:
 1. Install silently with the task "start at sign-in": files, shortcut,
    uninstall entry, Run value (in the sandbox key).
 2. Start the installed tray and install again (upgrade): the tray is quit
    through its pipe first, the file replaced, start at sign-in unchanged.
 3. Uninstall silently: everything is gone, the configuration and data are
    kept.
 4. Install again and uninstall with /REMOVEDATA: they are deleted too.

.PARAMETER Setup
The setup to test (scripts/installer.ps1 writes it).

.PARAMETER OnThisMachine
Run outside CI. The uninstall entry and the Start menu shortcut of this user
are created and removed again.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Setup,
    [switch]$OnThisMachine
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3

if (-not $env:CI -and -not $OnThisMachine) {
    throw 'this test creates an uninstall entry and a Start menu shortcut of this user; run it in CI or pass -OnThisMachine'
}

$AppId = '{0867F4F2-B652-4569-9343-104E8470E95F}'
$UninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\${AppId}_is1"
$Shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'kaltstellen.lnk'
$Setup = (Resolve-Path -LiteralPath $Setup).Path

if ((Test-Path -LiteralPath $UninstallKey) -or (Test-Path -LiteralPath $Shortcut)) {
    throw "kaltstellen is installed for this user already ($UninstallKey or $Shortcut); not touching it"
}

$root = Join-Path ([IO.Path]::GetTempPath()) ("ks-setup-test-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
# The uninstaller runs from a copy in is-*-uninstall.tmp, which it does not
# always remove; only folders that were not there before are this test's.
$innoTempBefore = @(Get-ChildItem -LiteralPath ([IO.Path]::GetTempPath()) -Directory -Filter 'is-*.tmp' -ErrorAction SilentlyContinue |
        ForEach-Object FullName)
$app = Join-Path $root 'app'
$cfg = Join-Path $root 'kaltstellen-config'
$data = Join-Path $root 'kaltstellen-data'
$runRoot = "Software\kaltstellen-setup-test-$PID"
$runKey = "$runRoot\Run"
$saved = @{}
foreach ($name in 'KALTSTELLEN_CONFIG_DIR', 'KALTSTELLEN_DATA_DIR', 'KALTSTELLEN_RUN_KEY', 'KALTSTELLEN_UPDATE', 'KALTSTELLEN_AUTOSTART_PROMPT') {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
# The setup, the tray and the uninstaller inherit these.
$env:KALTSTELLEN_CONFIG_DIR = $cfg
$env:KALTSTELLEN_DATA_DIR = $data
$env:KALTSTELLEN_RUN_KEY = $runKey
$env:KALTSTELLEN_UPDATE = 'off'
$env:KALTSTELLEN_AUTOSTART_PROMPT = 'skip'

function Assert([bool]$Condition, [string]$What) {
    if (-not $Condition) { throw "FAILED: $What" }
    Write-Host "ok: $What"
}

function Get-RunValue {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($runKey)
    if ($null -eq $k) { return $null }
    try { $k.GetValue('kaltstellen') } finally { $k.Close() }
}

function Invoke-Setup([string[]]$Arguments) {
    $log = Join-Path $root ("setup-" + [Guid]::NewGuid().ToString('N').Substring(0, 6) + '.log')
    $p = Start-Process -FilePath $Setup -Wait -PassThru -ArgumentList (@(
            '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$app`"", "/LOG=`"$log`"") + $Arguments)
    if ($p.ExitCode -ne 0) {
        if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log | Select-Object -Last 40 | Write-Host }
        throw "setup ended with exit code $($p.ExitCode)"
    }
}

# The uninstaller copies itself to %TEMP% and returns at once; wait until
# its work is visibly done.
function Invoke-Uninstall([string[]]$Arguments) {
    $un = Join-Path $app 'unins000.exe'
    Assert (Test-Path -LiteralPath $un) 'the uninstaller is there'
    $null = Start-Process -FilePath $un -Wait -PassThru -ArgumentList (@('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') + $Arguments)
    $deadline = (Get-Date).AddSeconds(90)
    while ((Test-Path -LiteralPath $UninstallKey) -or (Test-Path -LiteralPath $app)) {
        if ((Get-Date) -gt $deadline) { throw 'the uninstaller did not finish within 90 s' }
        Start-Sleep -Milliseconds 500
    }
}

function Stop-FromApp {
    Get-Process -ErrorAction SilentlyContinue | Where-Object {
        try { $_.Path -and $_.Path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase) } catch { $false }
    } | ForEach-Object {
        Write-Host "ending left-over $($_.Name) ($($_.Id))"
        Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
        $_.WaitForExit(10000) | Out-Null
    }
}

try {
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    $exe = Join-Path $app 'kaltstellen.exe'

    Write-Host '--- 1. first installation'
    Invoke-Setup @('/TASKS=autostart')
    Assert (Test-Path -LiteralPath $exe) 'kaltstellen.exe installed'
    # All of the output: a pipeline that stops early leaves no exit code.
    $out = @(& $exe --version)
    $code = $LASTEXITCODE
    $version = $out[0] -replace '^kaltstellen ', ''
    Assert ($code -eq 0 -and $version) "the installed program starts ($version)"
    Assert (Test-Path -LiteralPath $Shortcut) 'Start menu shortcut'
    $entry = Get-ItemProperty -LiteralPath $UninstallKey
    Assert ($entry.DisplayVersion -eq $version) "uninstall entry with version $($entry.DisplayVersion)"
    Assert ($entry.InstallLocation.TrimEnd('\') -eq $app) "uninstall entry points to $($entry.InstallLocation)"
    $run = Get-RunValue
    Assert ($null -ne $run -and $run -like "*$exe*") "start at sign-in set ($run)"
    $settings = Get-Content -Raw -LiteralPath (Join-Path $cfg 'settings.json')
    Assert ($settings -match '"autostart_asked"\s*:\s*true') 'the tray will not ask about start at sign-in again'

    Write-Host '--- 2. upgrade while the tray runs'
    $tray = Start-Process -FilePath $exe -ArgumentList 'tray' -PassThru -WindowStyle Hidden
    $logFile = Join-Path $data 'kaltstellen.log'
    $deadline = (Get-Date).AddSeconds(30)
    while (-not ((Test-Path -LiteralPath $logFile) -and (Select-String -LiteralPath $logFile -Pattern 'recover finished' -Quiet))) {
        if ($tray.HasExited) { throw "the tray ended at once (exit code $($tray.ExitCode))" }
        if ((Get-Date) -gt $deadline) { throw 'the tray did not come up within 30 s' }
        Start-Sleep -Milliseconds 300
    }
    # Turned off by hand since the first installation: an upgrade keeps it.
    & $exe autostart off | Out-Null
    Invoke-Setup @('/TASKS=autostart')
    Assert ($tray.WaitForExit(15000)) 'the setup quit the running tray'
    Assert (Test-Path -LiteralPath $exe) 'kaltstellen.exe still installed'
    Assert ($null -eq (Get-RunValue)) 'an upgrade leaves start at sign-in as it was'
    Assert (@(Get-ChildItem -LiteralPath (Split-Path -Parent $UninstallKey) | Where-Object PSChildName -like "*$AppId*").Count -eq 1) 'one uninstall entry'
    & $exe autostart on | Out-Null

    Write-Host '--- 3. uninstall, keeping the data'
    Invoke-Uninstall @()
    Assert (-not (Test-Path -LiteralPath $Shortcut)) 'shortcut removed'
    Assert ($null -eq (Get-RunValue)) 'start at sign-in removed'
    Assert ((Test-Path -LiteralPath $cfg) -and (Test-Path -LiteralPath $data)) 'configuration and data kept'

    Write-Host '--- 4. install again, uninstall with /REMOVEDATA'
    Invoke-Setup @('/TASKS=')
    Assert ($null -eq (Get-RunValue)) 'without the task, no start at sign-in'
    Invoke-Uninstall @('/REMOVEDATA')
    Assert (-not (Test-Path -LiteralPath $cfg) -and -not (Test-Path -LiteralPath $data)) 'configuration and data deleted'

    Write-Host 'installer test passed'
}
finally {
    Stop-FromApp
    # Only what this test created: the entry must point into its folder.
    if (Test-Path -LiteralPath $UninstallKey) {
        $loc = (Get-ItemProperty -LiteralPath $UninstallKey).InstallLocation
        if ($loc -and $loc.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $UninstallKey -Recurse -Force
            if (Test-Path -LiteralPath $Shortcut) { Remove-Item -LiteralPath $Shortcut -Force }
        }
    }
    [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($runRoot, $false)
    foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
    $deadline = (Get-Date).AddSeconds(20)
    do {
        $left = @(Get-ChildItem -LiteralPath ([IO.Path]::GetTempPath()) -Directory -Filter 'is-*.tmp' -ErrorAction SilentlyContinue |
                Where-Object { $innoTempBefore -notcontains $_.FullName })
        foreach ($dir in $left) { Remove-Item -LiteralPath $dir.FullName -Recurse -Force -ErrorAction SilentlyContinue }
        if ($left.Count -gt 0) { Start-Sleep -Milliseconds 500 }
    } while ($left.Count -gt 0 -and (Get-Date) -lt $deadline)
    if ($left.Count -gt 0) { Write-Warning "left behind: $($left.FullName -join ', ')" }
}
