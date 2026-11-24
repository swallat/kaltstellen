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
 2. With the tray running and the main window open, install again
    (upgrade): the window closes, the tray is quit through its pipe, the
    file replaced, the tray started again, start at sign-in unchanged.
 3. Uninstall silently: the tray ends, everything is gone, the profiles and
    data are kept.
 4. Install again and uninstall with /REMOVEDATA: kaltstellen's files are
    deleted too, a file of someone else in the same folder stays.

.PARAMETER Setup
The setup to test (scripts/installer.ps1 writes it).

.PARAMETER OnThisMachine
Run outside CI. The uninstall entry and the Start menu shortcut of this user
are created and removed again. The setup closes every open kaltstellen main
window (it finds them by window class), also one of another copy; the
window saves on every edit, so nothing is lost.

.PARAMETER SkipWindow
Leave out the main window in step 2 (no WebView2 runtime).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Setup,
    [switch]$OnThisMachine,
    [switch]$SkipWindow
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

Add-Type -Namespace KsSetupTest -Name User32 -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern IntPtr FindWindow(string cls, string title);
'@

$temp = [IO.Path]::GetTempPath()
$root = Join-Path $temp ("ks-setup-test-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
# The uninstaller runs from a copy in is-*-uninstall.tmp, which it does not
# always remove; only folders that were not there before are this test's.
$innoTempBefore = @(Get-ChildItem -LiteralPath $temp -Directory -Filter 'is-*.tmp' -ErrorAction SilentlyContinue |
        ForEach-Object FullName)
$app = Join-Path $root 'app'
$cfg = Join-Path $root 'kaltstellen-config'
$data = Join-Path $root 'kaltstellen-data'
$runRoot = "Software\kaltstellen-setup-test-$PID"
$runKey = "$runRoot\Run"
$exe = Join-Path $app 'kaltstellen.exe'
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

function Wait-Until([scriptblock]$Condition, [int]$Seconds, [string]$What) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while (-not (& $Condition)) {
        if ((Get-Date) -gt $deadline) { throw "FAILED: $What (waited $Seconds s)" }
        Start-Sleep -Milliseconds 300
    }
    Write-Host "ok: $What"
}

function Get-RunValue {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($runKey)
    if ($null -eq $k) { return $null }
    try { $k.GetValue('kaltstellen') } finally { $k.Close() }
}

function Get-Logged([string]$Text) {
    $log = Join-Path $data 'kaltstellen.log'
    if (-not (Test-Path -LiteralPath $log)) { return 0 }
    @(Select-String -LiteralPath $log -SimpleMatch -Pattern $Text).Count
}

function Get-FromRoot {
    @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
            try { $_.Path -and $_.Path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase) } catch { $false }
        })
}

function Get-InnoTemp {
    @(Get-ChildItem -LiteralPath $temp -Directory -Filter 'is-*.tmp' -ErrorAction SilentlyContinue |
            Where-Object { $innoTempBefore -notcontains $_.FullName })
}

function Invoke-Setup([string[]]$Arguments) {
    $log = Join-Path $root ("setup-" + [Guid]::NewGuid().ToString('N').Substring(0, 6) + '.log')
    # Not Start-Process -Wait: it also waits for the tray the setup starts.
    $p = Start-Process -FilePath $Setup -PassThru -ArgumentList (@(
            '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$app`"", "/LOG=`"$log`"") + $Arguments)
    $null = $p.Handle  # keeps the exit code readable
    if (-not $p.WaitForExit(300000)) { throw 'the setup did not end within 5 minutes' }
    if ($p.ExitCode -ne 0) {
        if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log | Select-Object -Last 40 | Write-Host }
        throw "setup ended with exit code $($p.ExitCode)"
    }
}

# The uninstaller copies itself to is-*-uninstall.tmp, starts that copy and
# returns at once; the copy removes the files and only then deletes the
# data (usPostUninstall). Done is when that copy has ended.
function Invoke-Uninstall([string[]]$Arguments) {
    $un = Join-Path $app 'unins000.exe'
    Assert (Test-Path -LiteralPath $un) 'the uninstaller is there'
    $log = Join-Path $root ("uninstall-" + [Guid]::NewGuid().ToString('N').Substring(0, 6) + '.log')
    $p = Start-Process -FilePath $un -PassThru -ArgumentList (@(
            '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"") + $Arguments)
    if (-not $p.WaitForExit(120000)) { throw 'the uninstaller did not end within 2 minutes' }
    Wait-Until {
        $copies = @(@(Get-InnoTemp) | ForEach-Object FullName)
        $running = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
                try { $p = $_.Path; $p -and @($copies | Where-Object { $p.StartsWith($_, [StringComparison]::OrdinalIgnoreCase) }).Count -gt 0 } catch { $false }
            })
        $running.Count -eq 0 -and -not (Test-Path -LiteralPath $UninstallKey)
    } 120 'the uninstaller finished'
}

function Start-Window {
    # A data folder of its own: its WebView2 files may stay locked for a
    # while after it closed (gui tests), and must not hold up step 4.
    $info = New-Object Diagnostics.ProcessStartInfo $exe, 'gui'
    $info.UseShellExecute = $false
    $info.EnvironmentVariables['KALTSTELLEN_DATA_DIR'] = (Join-Path $root 'window-data')
    [Diagnostics.Process]::Start($info)
}

try {
    New-Item -ItemType Directory -Force -Path $root, $cfg | Out-Null
    # A profile: the configuration is worth keeping (the uninstaller asks).
    Set-Content -LiteralPath (Join-Path $cfg 'config.toml') -Value "[[profile]]`nname = `"focus`"" -Encoding UTF8

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

    Write-Host '--- 2. upgrade while the tray runs and the window is open'
    $tray = Start-Process -FilePath $exe -ArgumentList 'tray' -PassThru -WindowStyle Hidden
    Wait-Until { $tray.HasExited -or (Get-Logged 'tray: recover finished') -ge 1 } 30 'the tray is up'
    Assert (-not $tray.HasExited) 'the tray runs'
    $window = $null
    if (-not $SkipWindow) {
        $window = Start-Window
        Wait-Until { $window.HasExited -or [KsSetupTest.User32]::FindWindow('kaltstellen-main', [NullString]::Value) -ne [IntPtr]::Zero } 60 'the main window is open'
        Assert (-not $window.HasExited) 'the main window runs'
    }
    # Turned off by hand since the first installation: an upgrade keeps it.
    & $exe autostart off | Out-Null
    Invoke-Setup @('/TASKS=autostart')
    Assert ($tray.WaitForExit(15000)) 'the setup quit the running tray'
    if ($window) { Assert ($window.WaitForExit(15000)) 'the setup closed the main window' }
    Assert (Test-Path -LiteralPath $exe) 'kaltstellen.exe still installed'
    Wait-Until { (Get-Logged 'tray: recover finished') -ge 2 } 30 'a silent upgrade started the tray again'
    Assert ($null -eq (Get-RunValue)) 'an upgrade leaves start at sign-in as it was'
    Assert (@(Get-ChildItem -LiteralPath (Split-Path -Parent $UninstallKey) | Where-Object PSChildName -like "*$AppId*").Count -eq 1) 'one uninstall entry'
    & $exe autostart on | Out-Null

    Write-Host '--- 3. uninstall, keeping the profiles and data'
    Invoke-Uninstall @()
    Assert (-not (Test-Path -LiteralPath $app)) 'program folder removed'
    Assert (@(Get-FromRoot).Count -eq 0) 'the tray ended'
    Assert (-not (Test-Path -LiteralPath $Shortcut)) 'shortcut removed'
    Assert ($null -eq (Get-RunValue)) 'start at sign-in removed'
    Assert ((Test-Path -LiteralPath (Join-Path $cfg 'config.toml')) -and (Test-Path -LiteralPath (Join-Path $data 'kaltstellen.log'))) 'profiles and data kept'

    Write-Host '--- 4. install again, uninstall with /REMOVEDATA'
    Invoke-Setup @('/TASKS=')
    Assert ($null -eq (Get-RunValue)) 'without the task, no start at sign-in'
    # Someone else's file in the (redirected) folder: never deleted.
    $foreign = Join-Path $cfg 'notes.txt'
    Set-Content -LiteralPath $foreign -Value 'not kaltstellen''s'
    Invoke-Uninstall @('/REMOVEDATA')
    Assert (-not (Test-Path -LiteralPath (Join-Path $cfg 'config.toml')) -and -not (Test-Path -LiteralPath (Join-Path $cfg 'settings.json'))) 'configuration deleted'
    Assert (-not (Test-Path -LiteralPath $data)) 'data folder deleted'
    Assert (Test-Path -LiteralPath $foreign) 'a file of someone else stays'

    Write-Host 'installer test passed'
}
finally {
    foreach ($p in @(Get-FromRoot)) {
        Write-Host "ending left-over $($p.Name) ($($p.Id))"
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        $p.WaitForExit(10000) | Out-Null
    }
    # Only what this test created: the entry and the shortcut must point
    # into its folder.
    if (Test-Path -LiteralPath $UninstallKey) {
        $loc = (Get-ItemProperty -LiteralPath $UninstallKey).InstallLocation
        if ($loc -and $loc.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $UninstallKey -Recurse -Force
        }
    }
    if (Test-Path -LiteralPath $Shortcut) {
        $target = (New-Object -ComObject WScript.Shell).CreateShortcut($Shortcut).TargetPath
        if ($target -and $target.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $Shortcut -Force
        }
    }
    [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($runRoot, $false)
    foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    # WebView2 lets go of its data folder up to two minutes after the window
    # closed (seen on the ARM64 runner).
    $deadline = (Get-Date).AddSeconds(150)
    while ((Test-Path -LiteralPath $root) -and (Get-Date) -lt $deadline) {
        Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $root) { Start-Sleep -Seconds 2 }
    }
    if (Test-Path -LiteralPath $root) { Write-Warning "left behind: $root" }
    $deadline = (Get-Date).AddSeconds(20)
    do {
        $left = @(Get-InnoTemp)
        foreach ($dir in $left) { Remove-Item -LiteralPath $dir.FullName -Recurse -Force -ErrorAction SilentlyContinue }
        if ($left.Count -gt 0) { Start-Sleep -Milliseconds 500 }
    } while ($left.Count -gt 0 -and (Get-Date) -lt $deadline)
    if ($left.Count -gt 0) { Write-Warning "left behind: $($left.FullName -join ', ')" }
}
