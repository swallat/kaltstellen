<#
.SYNOPSIS
Guided beta test of kaltstellen on the owner's machine (docs/beta-test.md).

.DESCRIPTION
Installs a kaltstellen release (or a local build), backs up the real
configuration, data folder and the start-at-sign-in value, writes a test
configuration built from the programs listed in beta-test.json, and walks
through the tests of docs/beta-test.md one step at a time: it prepares each
step through the kaltstellen command line, prints what to do, checks what a
script can check (log lines, `kaltstellen status`, frozen threads, window
answers, available memory) and asks about what only eyes can judge.
Results go to results.jsonl and results.log in the backup folder; `collect`
zips them with the matching log lines; `teardown` puts everything back.

Subcommands:
  setup     [-Version 1.0.0-beta.1] [-Exe <file>] [-InstallDir <dir>] [-Sandbox] [-DryRun]
  run       list | all | <test>
  collect   [-ReportDir <dir>]
  teardown  [-Uninstall] [-DryRun]
  status

Every prompt accepts r (resume everything frozen: `kaltstellen recover`
and `neutral`) and q (stop the run). The script never signs out or shuts
down unless you type the word it asks for. Its only network access is the
release download in `setup`.

.PARAMETER Command
setup, run, collect, teardown, status or help.

.PARAMETER Target
For run: a test id, `list` or `all`.

.PARAMETER Version
Release to download in setup (tag v<Version> of swallat/kaltstellen).

.PARAMETER Exe
Install this local kaltstellen.exe instead of downloading a release.

.PARAMETER InstallDir
Where kaltstellen.exe goes. Default %LOCALAPPDATA%\Programs\kaltstellen
(with -Sandbox: a folder inside the sandbox).

.PARAMETER Sandbox
Leave the real configuration, data folder and Run key alone: everything
lives in a temporary folder and a test registry key, which the script sets
through KALTSTELLEN_CONFIG_DIR, KALTSTELLEN_DATA_DIR and KALTSTELLEN_RUN_KEY
for everything it starts. Sign-in checks are n/a there.

.PARAMETER Uninstall
With teardown: also remove the installed executable (an earlier one that
setup replaced is put back).

.PARAMETER DryRun
With setup or teardown: print what would be done, change nothing. -WhatIf
works too.

.PARAMETER Answer
Answer every y/n question with this (y, n or s for skip). Recorded as a
preset answer, not as something seen.

.PARAMETER AnswersFile
JSON object mapping "<test>.<step>" (or "default") to y, n or s.

.PARAMETER Unattended
No waiting for Enter, steps that need a person are skipped, questions are
skipped unless -Answer or -AnswersFile says otherwise. For the probe run and
the automated smoke test.

.PARAMETER SimulateSessionEnd
For t7/t20: instead of signing out, send the tray the end-of-session
messages and end it, as Windows would. Tests the bookkeeping only.

.PARAMETER Restart
For t20: restart (shutdown /r) instead of shutting down.

.PARAMETER NoEdit
setup: do not open a new beta-test.json in Notepad.

.PARAMETER FreezeMinutes
t13: how long the programs stay frozen (default 10).

.PARAMETER SettleSeconds
t19: seconds of memory samples after each switch (default 20).

.PARAMETER ProgramList
Program list to use instead of <backup root>\beta-test.json.

.PARAMETER Root
Backup root instead of %LOCALAPPDATA%\kaltstellen-beta-backup (tests).

.PARAMETER ReportDir
Where collect writes the report zip. Default: the Desktop, or the backup
folder if there is no Desktop.

.EXAMPLE
.\scripts\beta-test.ps1 setup
.\scripts\beta-test.ps1 run list
.\scripts\beta-test.ps1 run tray
.\scripts\beta-test.ps1 collect
.\scripts\beta-test.ps1 teardown
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('setup', 'run', 'collect', 'teardown', 'status', 'help')]
    [string]$Command = 'help',
    [Parameter(Position = 1)]
    [string[]]$Target,
    [string]$Version = '1.0.0-beta.1',
    [string]$Exe,
    [string]$InstallDir,
    [switch]$Sandbox,
    [switch]$Uninstall,
    [Alias('WhatIf')]
    [switch]$DryRun,
    [ValidateSet('y', 'n', 's')]
    [string]$Answer,
    [string]$AnswersFile,
    [switch]$Unattended,
    [switch]$SimulateSessionEnd,
    [switch]$Restart,
    [string]$ProgramList,
    [string]$Root,
    [string]$ReportDir,
    [switch]$NoEdit,
    [double]$FreezeMinutes = 10,
    [int]$SettleSeconds = 20
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Off
$script:SavedModulePath = $env:PSModulePath
if ($PSVersionTable.PSEdition -ne 'Core') {
    # Windows PowerShell started from PowerShell 7 inherits its module path and
    # then misses its own modules (CimCmdlets, Defender): put them first.
    $own = Join-Path $PSHOME 'Modules'
    if (($env:PSModulePath -split ';') -notcontains $own) { $env:PSModulePath = "$own;$env:PSModulePath" }
}

$script:Repo = 'swallat/kaltstellen'
$script:StateFormat = 1
$script:Abort = 'KSBETA-ABORT'
$script:Invariant = [Globalization.CultureInfo]::InvariantCulture
$script:Utf8 = New-Object System.Text.UTF8Encoding $false
$script:EnvNames = @(
    'KALTSTELLEN_CONFIG_DIR', 'KALTSTELLEN_DATA_DIR', 'KALTSTELLEN_RUN_KEY',
    'KALTSTELLEN_AUTOSTART_PROMPT', 'KALTSTELLEN_UPDATE', 'KALTSTELLEN_NOTIFY'
)
$script:RealRunKey = 'Software\Microsoft\Windows\CurrentVersion\Run'
$script:RunValue = 'kaltstellen'
$script:SessionId = (Get-Process -Id $PID).SessionId
$script:BackupRoot = if ($Root) { [IO.Path]::GetFullPath($Root) } else { Join-Path $env:LOCALAPPDATA 'kaltstellen-beta-backup' }
$script:StateFile = Join-Path $script:BackupRoot 'beta-state.json'
$script:IsUnattended = [bool]$Unattended
$script:Answers = @{}
$script:LastAnswerSource = $null
$script:Ctx = @{}

# --- Windows API ------------------------------------------------------------

if (-not ('KsBeta.Native' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
namespace KsBeta {
    public class Win {
        public IntPtr Handle; public uint Pid; public string Class; public string Title; public bool Visible;
    }
    public static class Native {
        [DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, IntPtr l, uint f, uint t, out IntPtr r);
        [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
        [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
        [DllImport("user32.dll")] public static extern bool IsHungAppWindow(IntPtr h);
        [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
        delegate bool EnumProc(IntPtr h, IntPtr l);
        [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
        [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
        [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
        // WM_NULL with SMTO_ABORTIFHUNG: milliseconds until the window answered, -1 if it did not.
        public static long Ping(IntPtr h, uint timeoutMs) {
            IntPtr r; var sw = System.Diagnostics.Stopwatch.StartNew();
            var ok = SendMessageTimeout(h, 0, IntPtr.Zero, IntPtr.Zero, 2, timeoutMs, out r);
            return ok == IntPtr.Zero ? -1 : sw.ElapsedMilliseconds;
        }
        // A message the window must answer, with a timeout; -1 if it did not answer in time.
        public static long Send(IntPtr h, uint msg, long w, long l, uint timeoutMs) {
            IntPtr r; var sw = System.Diagnostics.Stopwatch.StartNew();
            var ok = SendMessageTimeout(h, msg, new IntPtr(w), new IntPtr(l), 2, timeoutMs, out r);
            return ok == IntPtr.Zero ? -1 : sw.ElapsedMilliseconds;
        }
        public static List<Win> Windows() {
            var list = new List<Win>();
            EnumWindows(delegate (IntPtr h, IntPtr x) {
                var c = new StringBuilder(256); var t = new StringBuilder(512); uint pid;
                GetClassName(h, c, 256); GetWindowText(h, t, 512); GetWindowThreadProcessId(h, out pid);
                var w = new Win(); w.Handle = h; w.Pid = pid; w.Class = c.ToString(); w.Title = t.ToString(); w.Visible = IsWindowVisible(h);
                list.Add(w); return true;
            }, IntPtr.Zero);
            return list;
        }
    }
    // Key presses through the keyboard's input queue (unattended hotkey check).
    public static class Keys {
        [StructLayout(LayoutKind.Sequential)] struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr extra; }
        // INPUT is a union whose largest member (MOUSEINPUT) is 8 bytes longer than KEYBDINPUT.
        [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public KEYBDINPUT ki; public ulong pad; }
        [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint n, INPUT[] inputs, int size);
        static INPUT Key(ushort vk, bool up) { var i = new INPUT(); i.type = 1; i.ki.wVk = vk; i.ki.dwFlags = up ? 2u : 0u; return i; }
        // Modifiers down, the key down `repeats` times (auto-repeat while held), all up.
        public static uint Chord(ushort[] modifiers, ushort key, int repeats) {
            var list = new List<INPUT>();
            foreach (var m in modifiers) list.Add(Key(m, false));
            for (int i = 0; i < repeats; i++) list.Add(Key(key, false));
            list.Add(Key(key, true));
            for (int i = modifiers.Length - 1; i >= 0; i--) list.Add(Key(modifiers[i], true));
            return SendInput((uint)list.Count, list.ToArray(), Marshal.SizeOf(typeof(INPUT)));
        }
    }
}
'@
}

# Virtual-key codes of a hotkey such as Ctrl+Shift+Alt+F9; $null if unknown.
function ConvertTo-VirtualKeys([string]$Hotkey) {
    $mods = New-Object System.Collections.Generic.List[uint16]
    $key = $null
    foreach ($part in $Hotkey.Split('+') | ForEach-Object { $_.Trim() }) {
        switch -Regex ($part) {
            '^(?i)(ctrl|control)$' { $mods.Add(0x11); continue }
            '^(?i)shift$' { $mods.Add(0x10); continue }
            '^(?i)alt$' { $mods.Add(0x12); continue }
            '^(?i)win$' { $mods.Add(0x5B); continue }
            '^(?i)f([1-9]|1[0-9]|2[0-4])$' { $key = [uint16](0x6F + [int]$Matches[1]); continue }
            '^[A-Za-z0-9]$' { $key = [uint16][char]$part.ToUpperInvariant(); continue }
            default { return $null }
        }
    }
    if ($null -eq $key) { return $null }
    @{ mods = [uint16[]]$mods.ToArray(); key = $key }
}
$script:WM_CLOSE = 0x0010
$script:WM_QUERYENDSESSION = 0x0011
$script:WM_ENDSESSION = 0x0016
$script:ENDSESSION_LOGOFF = [int64]2147483648

# --- Output -------------------------------------------------------------------

function Write-Info([string]$Text) { Write-Host $Text }
function Write-Good([string]$Text) { Write-Host $Text -ForegroundColor Green }
function Write-Note([string]$Text) { Write-Host $Text -ForegroundColor Yellow }
function Write-Bad([string]$Text) { Write-Host $Text -ForegroundColor Red }
function Write-Title([string]$Text) { Write-Host ''; Write-Host $Text -ForegroundColor Cyan }

function Get-UtcStamp([int]$OffsetMs = 0) {
    [DateTime]::UtcNow.AddMilliseconds($OffsetMs).ToString("yyyy-MM-dd'T'HH:mm:ss.fff'Z'", $script:Invariant)
}

function ConvertFrom-UtcStamp([string]$Stamp) {
    [DateTime]::ParseExact($Stamp.Substring(0, 23), 'yyyy-MM-ddTHH:mm:ss.fff', $script:Invariant,
        [Globalization.DateTimeStyles]::AssumeUniversal -bor [Globalization.DateTimeStyles]::AdjustToUniversal)
}

# PowerShell 7 turns ISO dates in JSON into DateTime values; 7.5 and later can
# be told not to. Timestamps are compared as text, so they are kept as text.
$script:JsonDateKind = (Get-Command ConvertFrom-Json).Parameters.ContainsKey('DateKind')

function ConvertFrom-JsonText([string]$Text) {
    if ($script:JsonDateKind) { ConvertFrom-Json -InputObject $Text -DateKind String }
    else { ConvertFrom-Json -InputObject $Text }
}

function ConvertTo-Stamp($Value) {
    if ($Value -is [DateTime]) { $Value.ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss.fff'Z'", $script:Invariant) }
    else { [string]$Value }
}

function Write-Utf8([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, $Text, $script:Utf8)
}

function Add-Utf8Line([string]$Path, [string]$Line) {
    [IO.File]::AppendAllText($Path, $Line + "`r`n", $script:Utf8)
}

function Get-List($Value) { @($Value | Where-Object { $null -ne $_ }) }

function Quote-Arg([string]$Arg) {
    if ($Arg -match '[\s"]') { '"' + ($Arg -replace '"', '\"') + '"' } else { $Arg }
}

# --- Safety -------------------------------------------------------------------

# Paths below which the script may delete things: what it created or backed up.
function Assert-Removable([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    $allowed = @()
    if ($script:State) {
        $allowed += Get-List @($script:State.temp_root, $script:State.config_dir, $script:State.data_dir)
        if ($script:State.install -and $script:State.install.created_dir) { $allowed += $script:State.install_dir }
    }
    $allowed += Join-Path $script:BackupRoot 'staging'
    foreach ($root in $allowed) {
        $r = [IO.Path]::GetFullPath($root).TrimEnd('\')
        if ($full -ieq $r -or $full.StartsWith($r + '\', [StringComparison]::OrdinalIgnoreCase)) { return }
    }
    throw "refusing to remove ${full}: not a path this script created or backed up"
}

function Remove-Tree([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    Assert-Removable $Path
    for ($i = 0; $i -lt 5; $i++) {
        try { Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop; return }
        catch { if ($i -eq 4) { throw }; Start-Sleep -Milliseconds 800 }
    }
}

function Clear-Folder([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    Assert-Removable $Path
    foreach ($child in Get-ChildItem -LiteralPath $Path -Force) { Remove-Tree $child.FullName }
}

# --- State --------------------------------------------------------------------

function Read-State {
    if (-not (Test-Path -LiteralPath $script:StateFile)) { return $null }
    $s = ConvertFrom-JsonText (Get-Content -LiteralPath $script:StateFile -Raw -Encoding UTF8)
    $s.setup_utc = ConvertTo-Stamp $s.setup_utc
    if ($s.pending) {
        $s.pending.since = ConvertTo-Stamp $s.pending.since
        $s.pending.boot = ConvertTo-Stamp $s.pending.boot
    }
    $s
}

function Save-State {
    $json = $script:State | ConvertTo-Json -Depth 8
    $tmp = $script:StateFile + '.tmp'
    Write-Utf8 $tmp $json
    Move-Item -LiteralPath $tmp -Destination $script:StateFile -Force
}

function Set-State([string]$Name, $Value) {
    $script:State | Add-Member -NotePropertyName $Name -NotePropertyValue $Value -Force
    Save-State
}

function Use-State {
    $script:State = Read-State
    if (-not $script:State) {
        throw "no beta test is set up in $($script:BackupRoot); run 'beta-test.ps1 setup' first"
    }
    if ($script:State.format -ne $script:StateFormat) { throw "state file $($script:StateFile) has an unknown format" }
    $script:KsExe = $script:State.exe
    $script:ConfigDir = $script:State.config_dir
    $script:DataDir = $script:State.data_dir
    $script:TestDir = $script:State.test_dir
    $script:Session = $script:State.session_dir
    if ($script:State.unattended) { $script:IsUnattended = $true }
    Set-KsEnvironment
    if ($script:State.programs_file -and (Test-Path -LiteralPath $script:State.programs_file)) {
        $script:Programs = Read-ProgramList $script:State.programs_file -NoFail
    }
}

# The environment of everything the script starts. Restored when it ends.
$script:SavedEnv = @{}
foreach ($n in $script:EnvNames) { $script:SavedEnv[$n] = [Environment]::GetEnvironmentVariable($n, 'Process') }

function Set-KsEnvironment {
    foreach ($n in $script:EnvNames) { [Environment]::SetEnvironmentVariable($n, $null, 'Process') }
    if ($script:State -and $script:State.env) {
        foreach ($p in $script:State.env.PSObject.Properties) { [Environment]::SetEnvironmentVariable($p.Name, [string]$p.Value, 'Process') }
    }
}

function Restore-Environment {
    foreach ($n in $script:EnvNames) { [Environment]::SetEnvironmentVariable($n, $script:SavedEnv[$n], 'Process') }
    $env:PSModulePath = $script:SavedModulePath
}

function Get-RunKey {
    if ($script:State -and $script:State.sandbox) { $script:State.env.KALTSTELLEN_RUN_KEY } else { $script:RealRunKey }
}

function Get-WayOutCommand {
    $cmd = "& '$($script:KsExe)' recover"
    if ($script:State -and $script:State.sandbox) {
        "`$env:KALTSTELLEN_CONFIG_DIR='$($script:ConfigDir)'; `$env:KALTSTELLEN_DATA_DIR='$($script:DataDir)'; `$env:KALTSTELLEN_RUN_KEY='$($script:State.env.KALTSTELLEN_RUN_KEY)'; $cmd"
    } else { $cmd }
}

# --- Prompts ------------------------------------------------------------------

function Read-AnswersFile {
    if (-not $AnswersFile) { return }
    $j = ConvertFrom-JsonText (Get-Content -LiteralPath $AnswersFile -Raw -Encoding UTF8)
    foreach ($p in $j.PSObject.Properties) { $script:Answers[$p.Name] = ([string]$p.Value).ToLowerInvariant() }
}

function Get-Preset([string]$Key) {
    if ($script:Answers.ContainsKey($Key)) { return $script:Answers[$Key] }
    $test = $Key.Split('.')[0]
    if ($script:Answers.ContainsKey($test)) { return $script:Answers[$test] }
    if ($Answer) { return $Answer }
    if ($script:Answers.ContainsKey('default')) { return $script:Answers['default'] }
    if ($script:IsUnattended) { return 's' }
    return $null
}

function Test-Abort($ErrorRecord) { $ErrorRecord.Exception.Message -eq $script:Abort }

function Invoke-WayOut {
    Write-Note '   Resuming everything (recover, then neutral) ...'
    $r = Invoke-Ks recover
    $r.Output | ForEach-Object { Write-Info "     $_" }
    $n = Invoke-Ks switch neutral
    $n.Output | ForEach-Object { Write-Info "     $_" }
}

function Read-Answer([string]$Key, [string]$Question) {
    $preset = Get-Preset $Key
    if ($preset) {
        $script:LastAnswerSource = 'preset'
        Write-Info "   ? $Question -> $preset (preset)"
        return $preset
    }
    $script:LastAnswerSource = 'person'
    while ($true) {
        $a = (Read-Host "   ? $Question [y]es / [n]o / [s]kip / [r]ecover / [q]uit").Trim().ToLowerInvariant()
        switch ($a) {
            'y' { return 'y' } 'yes' { return 'y' }
            'n' { return 'n' } 'no' { return 'n' }
            's' { return 's' } 'skip' { return 's' }
            'r' { Invoke-WayOut }
            'q' { Invoke-WayOut; throw $script:Abort }
            default { Write-Note '   Please answer y, n, s, r or q.' }
        }
    }
}

function Read-Note([string]$Key) {
    if ($script:IsUnattended -or (Get-Preset $Key)) { return '' }
    (Read-Host '   Note (what you saw; Enter for none)').Trim()
}

function Wait-Enter([string]$Key) {
    if ($script:IsUnattended) { return }
    while ($true) {
        $a = (Read-Host '   Press Enter when done (r = resume everything, q = stop this run)').Trim().ToLowerInvariant()
        if ($a -eq '') { return }
        if ($a -eq 'r') { Invoke-WayOut; continue }
        if ($a -eq 'q') { Invoke-WayOut; throw $script:Abort }
    }
}

function Confirm-Yes([string]$Question) {
    if ($script:IsUnattended) { return $Answer -eq 'y' }
    $a = (Read-Host "$Question [y/N]").Trim().ToLowerInvariant()
    $a -eq 'y' -or $a -eq 'yes'
}

function Show-FreezeWarning {
    Write-Note '   This step freezes or closes programs from beta-test.json: save your work in them first.'
    Write-Note "   Way out: answer r at any prompt, or in another terminal: $(Get-WayOutCommand)"
}

# --- Results ------------------------------------------------------------------

function Add-Result {
    param([string]$TestId, [string]$StepId, [string]$Outcome, $Auto, $Eyes, $Measure, [string]$Note, [string]$Since)
    $rec = [ordered]@{
        test = $TestId; step = $StepId; outcome = $Outcome
        auto = $Auto; eyes = $Eyes
        measure = $Measure; note = $Note
        since = $Since; until = (Get-UtcStamp); time = (Get-UtcStamp)
    }
    Add-Utf8Line (Join-Path $script:Session 'results.jsonl') ($rec | ConvertTo-Json -Depth 6 -Compress)
    $line = '{0}  {1,-12} {2,-16} {3,-5} auto={4} eyes={5}{6}' -f $rec.time, $TestId, $StepId, $Outcome, $Auto, $Eyes, $(if ($Note) { "  $Note" } else { '' })
    Add-Utf8Line (Join-Path $script:Session 'results.log') $line
    $color = switch ($Outcome) { 'pass' { 'Green' } 'fail' { 'Red' } default { 'DarkYellow' } }
    Write-Host ("   => {0}{1}" -f $Outcome, $(if ($Note) { ": $Note" } else { '' })) -ForegroundColor $color
}

function Get-Results {
    $file = Join-Path $script:Session 'results.jsonl'
    if (-not (Test-Path -LiteralPath $file)) { return @() }
    @(Get-Content -LiteralPath $file -Encoding UTF8 | Where-Object { $_.Trim() } | ForEach-Object {
        $r = ConvertFrom-JsonText $_
        foreach ($n in 'since', 'until', 'time') { $r.$n = ConvertTo-Stamp $r.$n }
        $r
    })
}

function Test-Dictionary($Value) { $Value -is [Collections.IDictionary] }

# Merges what a prepare or check block returned; other pipeline output is ignored.
function Merge-Measure($Into, $From) {
    foreach ($item in @($From)) {
        if (Test-Dictionary $item) { foreach ($k in @($item.Keys)) { $Into[$k] = $item[$k] } }
    }
}

function Format-Value($Value) {
    if ($null -eq $Value) { return '' }
    if ($Value -is [string] -or $Value -is [ValueType]) { $v = [string]$Value }
    else { $v = ConvertTo-Json -InputObject $Value -Depth 4 -Compress }
    if ($v.Length -gt 160) { $v = $v.Substring(0, 157) + '...' }
    $v
}

function Get-Outcome($Auto, $Eyes) {
    if ($Auto -eq $false -or $Eyes -eq $false) { return 'fail' }
    if ($Auto -eq $true -or $Eyes -eq $true) { return 'pass' }
    'skip'
}

# One step: prepare, tell, wait, check, ask, record.
function Invoke-Step {
    param(
        [Parameter(Mandatory = $true)][string]$TestId,
        [Parameter(Mandatory = $true)][string]$StepId,
        [Parameter(Mandatory = $true)][string]$Title,
        [scriptblock]$Prepare,
        [string[]]$Say,
        [scriptblock]$Check,
        [string]$Ask,
        [switch]$Freeze,
        [switch]$Human,
        [scriptblock]$Robot,
        [switch]$AskNote
    )
    $key = "$TestId.$StepId"
    Write-Title ("== {0} / {1}: {2}" -f $TestId, $StepId, $Title)
    $since = Get-UtcStamp (-100)
    $m = [ordered]@{}
    $notes = New-Object System.Collections.Generic.List[string]
    if ($Human -and $script:IsUnattended -and -not $Robot) {
        Add-Result $TestId $StepId 'skip' $null $null $m 'needs a person at the screen (unattended run)' $since
        return
    }
    if ($Freeze) { Show-FreezeWarning }
    $auto = $null; $eyes = $null; $override = $null
    try {
        if ($Prepare) { Merge-Measure $m (& $Prepare) }
        if ($Say) {
            foreach ($l in $Say) { Write-Info "   $l" }
            if ($script:IsUnattended -and $Robot) {
                Write-Note '   (unattended: the script does this step itself)'
                $null = & $Robot
            } else { Wait-Enter $key }
        } elseif ($script:IsUnattended -and $Robot) { $null = & $Robot }
        if ($Check) {
            $c = @(& $Check $since) | Where-Object { Test-Dictionary $_ } | Select-Object -Last 1
            if ($c) {
                $auto = $c.ok
                Merge-Measure $m $c.m
                if ($c.note) { $notes.Add([string]$c.note) }
                if ($c.outcome) { $override = $c.outcome }
            }
            $shown = if ($auto -eq $true) { 'pass' } elseif ($auto -eq $false) { 'FAIL' } else { 'recorded' }
            Write-Info "   automatic check: $shown"
            foreach ($k in $m.Keys) { Write-Host "     $k = $(Format-Value $m[$k])" -ForegroundColor DarkGray }
        }
    } catch {
        if (Test-Abort $_) { throw }
        $auto = $false
        $notes.Add('error: ' + $_.Exception.Message)
        Write-Bad "   error: $($_.Exception.Message)"
    }
    if ($Ask -and -not $override) {
        $a = Read-Answer $key $Ask
        $m['answer'] = $a
        $m['answered_by'] = $script:LastAnswerSource
        if ($a -eq 'y') { $eyes = $true } elseif ($a -eq 'n') { $eyes = $false }
        if ($a -eq 'n' -or ($AskNote -and $a -ne 's')) {
            $n = Read-Note $key
            if ($n) { $notes.Add($n) }
        }
    }
    $outcome = if ($override) { $override } else { Get-Outcome $auto $eyes }
    Add-Result $TestId $StepId $outcome $auto $eyes $m ($notes -join '; ') $since
}

function Add-Skip([string]$TestId, [string]$StepId, [string]$Why, [string]$Outcome = 'skip') {
    Write-Title "== $TestId / $StepId"
    Add-Result $TestId $StepId $Outcome $null $null ([ordered]@{}) $Why (Get-UtcStamp)
}

# --- kaltstellen --------------------------------------------------------------

function Assert-SandboxEnv {
    if ($script:State -and $script:State.sandbox -and $env:KALTSTELLEN_DATA_DIR -ne $script:State.data_dir) {
        throw 'internal error: sandbox variables are not set; refusing to run kaltstellen against the real configuration'
    }
}

function Invoke-Ks {
    Assert-SandboxEnv
    $eap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = @(& $script:KsExe @args 2>&1 | ForEach-Object { "$_" })
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $eap }
    [pscustomobject]@{ Code = $code; Output = $out; Text = ($out -join "`n") }
}

function Get-Active {
    $s = Invoke-Ks status
    if ($s.Text -match 'active profile: (.+)') { $Matches[1].Trim() } else { $null }
}

function Switch-Profile([string]$Name) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $r = Invoke-Ks switch $Name
    $r | Add-Member -NotePropertyName Ms -NotePropertyValue $sw.ElapsedMilliseconds
    $r
}

function Set-Neutral {
    if ((Get-Active) -ne 'neutral') { $null = Switch-Profile neutral }
}

function Get-OurProcesses {
    @(Get-Process -Name kaltstellen -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $script:SessionId -and $_.Path -and $_.Path -ieq $script:KsExe })
}

function Get-WindowsOf([int[]]$Pids, [string]$Class) {
    @([KsBeta.Native]::Windows() | Where-Object { ($Pids -contains [int]$_.Pid) -and (-not $Class -or $_.Class -eq $Class) })
}

function Get-TrayWindows {
    $ours = @(Get-OurProcesses | ForEach-Object { $_.Id })
    if (-not $ours.Count) { return @() }
    @(Get-WindowsOf $ours 'kaltstellen-tray')
}

function Get-HelperPids { @(Get-List $script:State.helper_pids | ForEach-Object { [int]$_ }) }

function Get-MainTray {
    $helpers = Get-HelperPids
    Get-TrayWindows | Where-Object { $helpers -notcontains [int]$_.Pid } | Select-Object -First 1
}

function Wait-Until([double]$Seconds, [scriptblock]$Condition, [int]$StepMs = 200) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $Seconds) {
        $v = & $Condition
        if ($v) { return $v }
        Start-Sleep -Milliseconds $StepMs
    }
    & $Condition
}

function Start-Tray {
    $t = Get-MainTray
    if ($t) { return $t }
    Assert-SandboxEnv
    $since = Get-UtcStamp (-500)
    $p = Start-Process -FilePath $script:KsExe -ArgumentList 'tray' -WindowStyle Hidden -PassThru
    $t = Wait-Until 20 { Get-MainTray }
    $null = Wait-Log 'tray: pipe ready' $since 10
    if (-not $t) { throw "the tray did not start (pid $($p.Id)); see $($script:DataDir)\kaltstellen.log" }
    $t
}

function Stop-TrayWindow($Tray, [int]$TimeoutS = 25) {
    if (-not $Tray) { return $true }
    $null = [KsBeta.Native]::PostMessage($Tray.Handle, $script:WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
    $gone = Wait-Until $TimeoutS { -not (Get-Process -Id $Tray.Pid -ErrorAction SilentlyContinue) }
    if (-not $gone) {
        Write-Note "   the tray (pid $($Tray.Pid)) did not quit in $TimeoutS s; ending it"
        Stop-Process -Id $Tray.Pid -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 500
        return $false
    }
    $true
}

function Stop-Tray {
    $t = Get-MainTray
    if ($t) { $null = Stop-TrayWindow $t }
}

function Get-LogLines([string]$Since, [string]$Until) {
    $files = @('kaltstellen.log.2', 'kaltstellen.log.1', 'kaltstellen.log') | ForEach-Object { Join-Path $script:DataDir $_ } | Where-Object { Test-Path -LiteralPath $_ }
    $out = New-Object System.Collections.Generic.List[string]
    foreach ($f in $files) {
        $lines = $null
        try {
            $fs = [IO.File]::Open($f, 'Open', 'Read', 'ReadWrite, Delete')
            try { $lines = (New-Object IO.StreamReader($fs, $script:Utf8)).ReadToEnd() -split "`r?`n" } finally { $fs.Dispose() }
        } catch { continue }
        $take = $false
        foreach ($l in $lines) {
            if ($l.Length -ge 24 -and $l[4] -eq '-' -and $l[10] -eq 'T') {
                $ts = $l.Substring(0, 24)
                $take = ([string]::CompareOrdinal($ts, $Since) -ge 0) -and (-not $Until -or [string]::CompareOrdinal($ts, $Until) -le 0)
            }
            if ($take -and $l) { $out.Add($l) }
        }
    }
    , $out.ToArray()
}

function Find-Log([string]$Pattern, [string]$Since, [switch]$Regex, [switch]$All) {
    $lines = Get-LogLines $Since
    $hits = if ($Regex) { @($lines | Where-Object { $_ -match $Pattern }) } else { @($lines | Where-Object { $_.Contains($Pattern) }) }
    if ($All) { return , $hits }
    $hits | Select-Object -First 1
}

function Wait-Log([string]$Pattern, [string]$Since, [double]$Seconds = 10, [switch]$Regex) {
    Wait-Until $Seconds { Find-Log $Pattern $Since -Regex:$Regex } 300
}

function Get-LogTime([string]$Line) { if ($Line) { ConvertFrom-UtcStamp $Line } else { $null } }

function Read-JsonFile([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return $null }
    try { ConvertFrom-JsonText (Get-Content -LiteralPath $Path -Raw -Encoding UTF8) } catch { $null }
}

function Set-SettingsValue([string]$Name, $Value) {
    $file = Join-Path $script:ConfigDir 'settings.json'
    $s = Read-JsonFile $file
    if (-not $s) { $s = New-Object psobject }
    if ($null -eq $Value) { $s.PSObject.Properties.Remove($Name) }
    else { $s | Add-Member -NotePropertyName $Name -NotePropertyValue $Value -Force }
    Write-Utf8 $file ($s | ConvertTo-Json -Depth 4)
}

# Automatic switching on or off: written to settings.json with the tray quit,
# read by the tray when it starts again (setup starts it paused, so a trigger
# program you happen to start does not freeze anything before test 'triggers').
function Set-TriggersPaused([bool]$Paused) {
    $wasRunning = [bool](Get-MainTray)
    if ($wasRunning) { Set-Neutral; Stop-Tray }
    Set-SettingsValue 'triggers_paused' $(if ($Paused) { $true } else { $null })
    if ($wasRunning -or -not $Paused) { $null = Start-Tray }
}

# --- Programs -----------------------------------------------------------------

function Get-ProgramProcesses([string]$ExeName) {
    $base = [IO.Path]::GetFileNameWithoutExtension($ExeName)
    @(Get-Process -Name $base -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $script:SessionId })
}

function Test-ProcessFrozen($Process) {
    try {
        $Process.Refresh()
        $threads = @($Process.Threads)
        if (-not $threads.Count) { return $null }
        foreach ($t in $threads) {
            try { if ($t.ThreadState -ne 'Wait' -or $t.WaitReason -ne 'Suspended') { return $false } } catch { return $false }
        }
        $true
    } catch { $null }
}

# Per program: frozen, running, not running or unreadable.
function Get-FreezeReport($Programs) {
    $detail = [ordered]@{}
    $all = $true; $any = $false
    foreach ($p in Get-List $Programs) {
        $procs = Get-ProgramProcesses $p.exe
        if (-not $procs.Count) { $detail[$p.exe] = 'not running'; continue }
        $states = @($procs | ForEach-Object { Test-ProcessFrozen $_ })
        if ($states -contains $false) {
            # Which threads run: a thread Windows adds to a frozen process shows here too.
            $counts = @($procs | ForEach-Object {
                $t = @($_.Threads); $n = 0
                foreach ($th in $t) { try { if ($th.ThreadState -eq 'Wait' -and $th.WaitReason -eq 'Suspended') { $n++ } } catch {} }
                "$n/$($t.Count) threads suspended"
            })
            $detail[$p.exe] = "running ($($counts -join ', '))"; $all = $false
        }
        elseif ($states -contains $true) { $detail[$p.exe] = "frozen ($($procs.Count))"; $any = $true }
        else { $detail[$p.exe] = 'unreadable' }
    }
    @{ all = ($all -and $any); none = (-not $any); detail = $detail }
}

function Get-BeatFile($Program) { Join-Path (Join-Path $script:TestDir 'beats') ($Program.exe + '.beat') }

function Get-Beat($Program) {
    try { [int64]([IO.File]::ReadAllText((Get-BeatFile $Program)).Trim()) } catch { 0 }
}

function Register-Started($Process, $Program) {
    $entry = [pscustomobject]@{
        pid = $Process.Id; exe = $Program.exe; path = $Program.path
        start_ticks = $(try { $Process.StartTime.ToUniversalTime().Ticks } catch { 0 })
        disposable = [bool]$Program.disposable
    }
    Set-State 'started' (@(Get-List $script:State.started) + @($entry))
}

function Start-Program($Program, [switch]$Hidden) {
    $argList = @(Get-List $Program.args | ForEach-Object { Quote-Arg (([string]$_).Replace('{heartbeat}', (Get-BeatFile $Program))) })
    $params = @{ FilePath = $Program.path; PassThru = $true }
    if ($argList.Count) { $params.ArgumentList = $argList }
    if ($Hidden -or $Program.hidden) { $params.WindowStyle = 'Hidden' }
    $p = Start-Process @params
    Register-Started $p $Program
    $null = Wait-Until 15 { (Get-ProgramProcesses $Program.exe).Count -gt 0 }
    $p
}

# Makes sure the listed programs run: starts those with a path, asks for the
# others. Returns the programs that run.
function Request-Running($Programs, [int]$SettleMs = 1500) {
    $started = $false
    $running = @()
    foreach ($p in Get-List $Programs) {
        if ((Get-ProgramProcesses $p.exe).Count) { $running += $p; continue }
        if ($p.path) {
            Write-Info "   starting $($p.exe)"
            $null = Start-Program $p
            $started = $true
            $running += $p
            continue
        }
        if ($script:IsUnattended) { Write-Note "   $($p.exe) is not running and has no path; left out"; continue }
        while (-not (Get-ProgramProcesses $p.exe).Count) {
            $a = (Read-Host "   Start $($p.exe) now, then press Enter (s = leave it out)").Trim().ToLowerInvariant()
            if ($a -eq 's') { break }
        }
        if ((Get-ProgramProcesses $p.exe).Count) { $running += $p }
    }
    if ($started) { Start-Sleep -Milliseconds $SettleMs }
    , $running
}

# Ends a program the script started: killed when disposable, otherwise asked
# to close (WM_CLOSE to its windows).
function Close-Program($Program, [switch]$Wait) {
    $procs = Get-ProgramProcesses $Program.exe
    if (-not $procs.Count) { return $true }
    if ($Program.disposable) { $procs | Stop-Process -Force -ErrorAction SilentlyContinue }
    else { foreach ($p in $procs) { $null = $p.CloseMainWindow() } }
    if (-not $Wait) { return $true }
    [bool](Wait-Until 15 { -not (Get-ProgramProcesses $Program.exe).Count })
}

function Request-Closed($Program) {
    if (-not (Get-ProgramProcesses $Program.exe).Count) { return $true }
    $ours = @(Get-List $script:State.started | Where-Object { $_.exe -ieq $Program.exe })
    if ($Program.disposable -or $ours.Count) {
        if (Close-Program $Program -Wait) { return $true }
    }
    if ($script:IsUnattended) { return $false }
    while ((Get-ProgramProcesses $Program.exe).Count) {
        $a = (Read-Host "   Close $($Program.exe), then press Enter (s = go on anyway)").Trim().ToLowerInvariant()
        if ($a -eq 's') { return $false }
    }
    $true
}

function Get-MainWindows($Program) {
    $pids = @(Get-ProgramProcesses $Program.exe | ForEach-Object { $_.Id })
    if (-not $pids.Count) { return @() }
    @(Get-WindowsOf $pids | Where-Object { $_.Visible -and $_.Title })
}

function Get-AvailableMb {
    try {
        $m = Get-CimInstance Win32_PerfRawData_PerfOS_Memory -ErrorAction Stop
        [math]::Round($m.AvailableBytes / 1MB, 1)
    } catch {
        $o = Get-CimInstance Win32_OperatingSystem
        [math]::Round($o.FreePhysicalMemory / 1KB, 1)
    }
}

function Get-AverageAvailableMb([int]$Samples = 3) {
    $v = @()
    for ($i = 0; $i -lt $Samples; $i++) { $v += Get-AvailableMb; if ($i -lt $Samples - 1) { Start-Sleep -Milliseconds 700 } }
    [math]::Round(($v | Measure-Object -Average).Average, 1)
}

function Get-WorkingSetMb([string]$ExeName) {
    $sum = 0
    foreach ($p in Get-ProgramProcesses $ExeName) { try { $p.Refresh(); $sum += $p.WorkingSet64 } catch {} }
    [math]::Round($sum / 1MB, 1)
}

function Test-Admin {
    ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Get-RunValue([string]$Key) {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Key)
    if (-not $k) { return [pscustomobject]@{ present = $false; kind = $null; data = $null } }
    try {
        $v = $k.GetValue($script:RunValue, $null, 'DoNotExpandEnvironmentNames')
        if ($null -eq $v) { return [pscustomobject]@{ present = $false; kind = $null; data = $null } }
        [pscustomobject]@{ present = $true; kind = [string]$k.GetValueKind($script:RunValue); data = [string]$v }
    } finally { $k.Close() }
}

# --- Program list and test configuration ---------------------------------------

function Read-ProgramList([string]$Path, [switch]$NoFail) {
    $errors = New-Object System.Collections.Generic.List[string]
    try { $j = ConvertFrom-JsonText (Get-Content -LiteralPath $Path -Raw -Encoding UTF8) }
    catch { throw "$Path is not valid JSON: $($_.Exception.Message)" }
    $seen = @{}
    $norm = {
        param($p, $role)
        if ($null -eq $p) { return $null }
        $exe = [string]$p.exe
        if (-not $exe -or $exe -match '[<>]') { $errors.Add("$role`: replace the placeholder in exe ('$exe')"); return $null }
        if ($exe -match '[\\/]' -or $exe -notmatch '\.exe$') { $errors.Add("$role`: exe must be a file name ending in .exe ('$exe')"); return $null }
        if ($exe -ieq 'kaltstellen.exe') { $errors.Add("$role`: kaltstellen.exe cannot be a test program"); return $null }
        $path = [string]$p.path
        if ($path -match '[<>]') { $errors.Add("$role $exe`: replace the placeholder in path"); $path = '' }
        if ($path) {
            $path = [Environment]::ExpandEnvironmentVariables($path)
            if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { $errors.Add("$role $exe`: path not found: $path") }
        }
        [pscustomobject]@{
            exe = $exe; path = $path; args = @(Get-List $p.args | ForEach-Object { [string]$_ })
            disposable = [bool]$p.disposable; hidden = [bool]$p.hidden
            restart = $(if ($null -eq $p.restart) { $true } else { [bool]$p.restart })
            on_timeout = $(if ($p.on_timeout -eq 'kill') { 'kill' } else { 'keep' })
            role = $role
        }
    }
    $suspend = @(Get-List $j.suspend | ForEach-Object { & $norm $_ 'suspend' } | Where-Object { $_ })
    $stop = @(Get-List $j.stop | ForEach-Object { & $norm $_ 'stop' } | Where-Object { $_ })
    if (-not $suspend.Count) { $errors.Add('suspend: list at least one program that may be frozen') }
    foreach ($p in $suspend + $stop) {
        $k = $p.exe.ToLowerInvariant()
        if ($seen.ContainsKey($k)) { $errors.Add("$($p.exe) is listed twice (suspend and stop exclude each other)") }
        $seen[$k] = $p.role
    }
    $trigger = & $norm $j.trigger 'trigger'
    if ($trigger -and $seen.ContainsKey($trigger.exe.ToLowerInvariant())) { $errors.Add("trigger $($trigger.exe) must not be frozen or stopped (it is in $($seen[$trigger.exe.ToLowerInvariant()]))") }
    $large = & $norm $j.large 'large'
    if ($large -and $seen[$large.exe.ToLowerInvariant()] -eq 'stop') { $errors.Add("large $($large.exe) is in stop; it must be a program that may be frozen") }
    if ($large -and $trigger -and $large.exe -ieq $trigger.exe) { $errors.Add('large and trigger must be different programs') }
    $elevated = & $norm $j.elevated 'elevated'
    $tray = & $norm $j.tray_program 'tray_program'
    if ($tray -and $seen.ContainsKey($tray.exe.ToLowerInvariant())) { $errors.Add("tray_program $($tray.exe) must not be in a profile (it is in $($seen[$tray.exe.ToLowerInvariant()]))") }
    $hotkey = if ($j.hotkey) { [string]$j.hotkey } else { 'Ctrl+Shift+Alt+F9' }
    $altgr = if ($j.altgr_hotkey) { [string]$j.altgr_hotkey } else { 'Ctrl+Alt+7' }
    if ($errors.Count -and -not $NoFail) {
        throw ("$Path has problems:`n  " + ($errors -join "`n  "))
    }
    [pscustomobject]@{
        suspend = $suspend; stop = $stop; trigger = $trigger; large = $large
        elevated = $elevated; tray_program = $tray; hotkey = $hotkey; altgr_hotkey = $altgr
    }
}

function ConvertTo-TomlString([string]$Text) { '"' + $Text.Replace('\', '\\').Replace('"', '\"') + '"' }

function New-TestConfig($P, [switch]$TriggerSelfTest) {
    $sb = New-Object System.Text.StringBuilder
    $line = { param($t) $null = $sb.AppendLine($t) }
    $app = {
        param([string]$ExeName, [string]$Action, [hashtable]$Extra)
        & $line ''
        & $line '  [[profile.app]]'
        & $line "  exe = $(ConvertTo-TomlString $ExeName)"
        & $line "  action = `"$Action`""
        if ($Extra) {
            foreach ($k in ($Extra.Keys | Sort-Object)) {
                $v = $Extra[$k]
                $txt = if ($v -is [bool]) { $v.ToString().ToLowerInvariant() } elseif ($v -is [int]) { "$v" } else { ConvertTo-TomlString ([string]$v) }
                & $line "  $k = $txt"
            }
        }
    }
    $prof = {
        param([string]$Name, [string]$Comment, [string[]]$Header)
        & $line ''
        & $line "# $Comment"
        & $line '[[profile]]'
        & $line "name = `"$Name`""
        foreach ($h in $Header) { & $line $h }
    }
    $first = $P.suspend[0]
    & $line '# Test configuration of the kaltstellen beta test, written by scripts/beta-test.ps1.'
    & $line '# "beta-test.ps1 teardown" puts your own configuration back.'
    & $line '# beta-test: generated'
    & $line ''
    & $line '[settings]'
    & $line 'stop_timeout_s = 8'

    & $prof 'beta-freeze' 'Freezes every program of the "suspend" list (tray, hotkeys, T13, T14).' @("hotkey = $(ConvertTo-TomlString $P.hotkey)")
    foreach ($s in $P.suspend) { & $app $s.exe 'suspend' $null }

    if ($P.stop.Count) {
        & $prof 'beta-stop' 'Closes the programs of the "stop" list; neutral starts them again.' @()
        foreach ($s in $P.stop) {
            $extra = @{ on_timeout = $s.on_timeout }
            if ($s.restart) { $extra.restart_on_neutral = $true }
            & $app $s.exe 'stop' $extra
        }
    }

    $missing = Join-Path (Join-Path $script:TestDir 'missing') 'ks-beta-missing.exe'
    & $prof 'beta-failure' 'A start entry whose program does not exist yet (failure markers, 13c).' @()
    & $app 'ks-beta-missing.exe' 'start' @{ path = $missing }
    & $app $first.exe 'suspend' $null

    & $prof 'beta-dragdrop' 'Empty; drop files from Explorer onto it (13d).' @()

    if ($P.trigger) {
        & $prof 'beta-trigger' "Applied while $($P.trigger.exe) runs (automatic triggers)." @("trigger = [$(ConvertTo-TomlString $P.trigger.exe)]")
        & $app $first.exe 'suspend' $null
        if ($TriggerSelfTest) { & $app $P.trigger.exe 'suspend' $null }
    }

    if ($P.large) {
        & $prof 'beta-trim-on' "Freezes $($P.large.exe) and moves its memory out of RAM (T19)." @()
        & $app $P.large.exe 'suspend' @{ trim = $true }
        & $prof 'beta-trim-off' "Freezes $($P.large.exe) and leaves its memory (T19)." @()
        & $app $P.large.exe 'suspend' @{ trim = $false }
    }

    if ($P.elevated) {
        & $prof 'beta-elevated' "Freezes $($P.elevated.exe) (run as administrator: reported) and $($first.exe) (T18)." @()
        & $app $P.elevated.exe 'suspend' $null
        & $app $first.exe 'suspend' $null
    }

    & $prof 'beta-session' 'Freezes the "suspend" list and closes the "stop" list (T7, T20).' @()
    foreach ($s in $P.suspend) { & $app $s.exe 'suspend' $null }
    foreach ($s in $P.stop) { & $app $s.exe 'stop' @{ on_timeout = $s.on_timeout } }

    & $prof 'beta-altgr' 'Ctrl+Alt with a digit: refused, AltGr keeps typing (T17).' @("hotkey = $(ConvertTo-TomlString $P.altgr_hotkey)")
    $sb.ToString()

}

function New-SampleFiles([string]$Dir) {
    $drop = Join-Path $Dir 'dragdrop'
    New-Item -ItemType Directory -Force -Path $drop, (Join-Path $Dir 'missing'), (Join-Path $Dir 'beats'), (Join-Path $drop 'a folder') | Out-Null
    $ping = Join-Path $env:SystemRoot 'System32\PING.EXE'
    foreach ($n in 'ks-beta-drop1.exe', 'ks-beta-drop2.exe', 'ks-beta-drop3.exe', 'anydesk.exe') { Copy-Item -LiteralPath $ping -Destination (Join-Path $drop $n) }
    Write-Utf8 (Join-Path $drop 'notes.txt') "A text file for the drag and drop test; kaltstellen refuses it.`r`n"
    $shell = New-Object -ComObject WScript.Shell
    $lnk = $shell.CreateShortcut((Join-Path $drop 'shortcut to drop1.lnk'))
    $lnk.TargetPath = Join-Path $drop 'ks-beta-drop1.exe'
    $lnk.Save()
    $tray = $shell.CreateShortcut((Join-Path $Dir 'kaltstellen tray.lnk'))
    $tray.TargetPath = $script:KsExe
    $tray.Arguments = 'tray'
    $tray.Save()
    $null = [Runtime.InteropServices.Marshal]::ReleaseComObject($shell)
}

# --- setup --------------------------------------------------------------------

function Get-Arch {
    $a = $null
    try { $a = [string][Runtime.InteropServices.RuntimeInformation]::OSArchitecture } catch {}
    if (-not $a) { $a = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE } }
    switch -Regex ($a) {
        '^(X64|AMD64)$' { 'x86_64' }
        '^(Arm64|ARM64)$' { 'aarch64' }
        default { throw "unsupported architecture '$a': releases exist for x86_64 and aarch64" }
    }
}

# .NET rather than Get-FileHash: Windows PowerShell started from PowerShell 7
# or another program may inherit a module path without its own modules.
function Get-Sha256([string]$Path) {
    $sha = [Security.Cryptography.SHA256]::Create()
    $fs = [IO.File]::Open($Path, 'Open', 'Read', 'ReadWrite')
    try { -join ($sha.ComputeHash($fs) | ForEach-Object { $_.ToString('x2') }) } finally { $fs.Dispose(); $sha.Dispose() }
}

function Get-Manifest([string]$Dir) {
    if (-not (Test-Path -LiteralPath $Dir)) { return @() }
    $base = (Resolve-Path -LiteralPath $Dir).ProviderPath.TrimEnd('\')
    @(Get-ChildItem -LiteralPath $Dir -Recurse -Force -File | ForEach-Object {
        [pscustomobject]@{ rel = $_.FullName.Substring($base.Length + 1); size = $_.Length; sha256 = (Get-Sha256 $_.FullName) }
    })
}

function Test-Manifest([string]$Dir, $Manifest) {
    $now = @{}
    foreach ($f in Get-Manifest $Dir) { $now[$f.rel.ToLowerInvariant()] = $f.sha256 }
    $want = @(Get-List $Manifest)
    if ($now.Count -ne $want.Count) { return "expected $($want.Count) files, found $($now.Count)" }
    foreach ($f in $want) {
        $h = $now[$f.rel.ToLowerInvariant()]
        if ($h -ne $f.sha256) { return "$($f.rel) differs" }
    }
    $null
}

function Backup-Folder([string]$Source, [string]$Dest) {
    $existed = Test-Path -LiteralPath $Source
    $manifest = @()
    if ($existed) {
        $manifest = Get-Manifest $Source
        New-Item -ItemType Directory -Force -Path (Split-Path $Dest) | Out-Null
        Copy-Item -LiteralPath $Source -Destination $Dest -Recurse -Force
        if (-not (Test-Path -LiteralPath $Dest)) { New-Item -ItemType Directory -Path $Dest | Out-Null }
        $problem = Test-Manifest $Dest $manifest
        if ($problem) { throw "backup of $Source could not be verified: $problem" }
    }
    [pscustomobject]@{ path = $Source; existed = $existed; backup = $Dest; files = $manifest }
}

function Get-ReleaseFile([string]$Url, [string]$OutFile) {
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {}
    $old = $ProgressPreference
    $ProgressPreference = 'SilentlyContinue'
    try { Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing }
    catch { throw "download failed: $Url ($($_.Exception.Message))" }
    finally { $ProgressPreference = $old }
}

function Get-RunningKaltstellen {
    @(Get-Process -Name kaltstellen -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $script:SessionId })
}

function Invoke-Setup {
    $existing = Read-State
    if ($existing) {
        throw "a beta test is already set up (phase '$($existing.phase)', $($existing.session_dir)); run 'beta-test.ps1 teardown' first"
    }
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $arch = Get-Arch
    $tempRoot = Join-Path ([IO.Path]::GetTempPath()) "kaltstellen-beta-$stamp"
    $sessionDir = Join-Path $script:BackupRoot $stamp
    $realConfig = Join-Path $env:APPDATA 'kaltstellen'
    $realData = Join-Path $env:LOCALAPPDATA 'kaltstellen'
    $install = if ($InstallDir) { [IO.Path]::GetFullPath($InstallDir) }
               elseif ($Sandbox) { Join-Path $tempRoot 'bin' }
               else { Join-Path $env:LOCALAPPDATA 'Programs\kaltstellen' }
    if ($install -match '(?i)\\Program Files( \(x86\))?\\|\\scoop\\apps\\') { Write-Note "Note: the updater refuses to replace a copy in $install." }
    $configDir = if ($Sandbox) { Join-Path $tempRoot 'config' } else { $realConfig }
    $dataDir = if ($Sandbox) { Join-Path $tempRoot 'data' } else { $realData }
    $regRoot = "Software\kaltstellen-beta-$stamp"
    $source = if ($Exe) { [IO.Path]::GetFullPath($Exe) } else { "https://github.com/$($script:Repo)/releases/download/v$Version/kaltstellen-$Version-windows-$arch.exe" }
    if ($Exe -and -not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "-Exe: file not found: $source" }

    # The program list first: nothing changes while it is not usable.
    $list = if ($ProgramList) { [IO.Path]::GetFullPath($ProgramList) } else { Join-Path $script:BackupRoot 'beta-test.json' }
    if (-not (Test-Path -LiteralPath $list)) {
        if ($DryRun) { Write-Info "[dry run] would copy beta-test.example.json to $list for editing" }
        else {
            New-Item -ItemType Directory -Force -Path (Split-Path $list) | Out-Null
            Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'beta-test.example.json') -Destination $list
            Write-Note "Created $list from the example. List the programs the test may freeze or stop there."
            if (-not $NoEdit -and -not $script:IsUnattended) {
                Start-Process notepad.exe -ArgumentList (Quote-Arg $list)
                $null = Read-Host 'Edit and save the file, then press Enter'
            } else {
                Write-Note 'Edit it, then run setup again.'
                return
            }
        }
    }
    $programs = $null
    if (Test-Path -LiteralPath $list) { $programs = Read-ProgramList $list }

    $running = Get-RunningKaltstellen
    $runKey = if ($Sandbox) { "$regRoot\Run" } else { $script:RealRunKey }
    $runBefore = Get-RunValue $script:RealRunKey

    Write-Title 'Plan'
    Write-Info "  mode:            $(if ($Sandbox) { 'sandbox (real configuration untouched)' } else { 'real configuration (backed up first)' })"
    Write-Info "  executable:      $source"
    Write-Info "  install to:      $install\kaltstellen.exe$(if (Test-Path -LiteralPath (Join-Path $install 'kaltstellen.exe')) { ' (an existing one is backed up)' })"
    Write-Info "  configuration:   $configDir$(if (-not $Sandbox) { $(if (Test-Path -LiteralPath $realConfig) { ' (exists: backed up, then replaced by the test configuration)' } else { ' (does not exist: created, removed at teardown)' }) })"
    Write-Info "  data:            $dataDir$(if (-not $Sandbox) { $(if (Test-Path -LiteralPath $realData) { ' (exists: backed up, restored at teardown)' } else { ' (does not exist: removed at teardown)' }) })"
    Write-Info "  Run value:       HKCU\$runKey\kaltstellen$(if (-not $Sandbox) { " (now: $(if ($runBefore.present) { $runBefore.data } else { 'absent' }); restored at teardown)" })"
    Write-Info "  test files:      $tempRoot"
    Write-Info "  backup, results: $sessionDir"
    Write-Info "  program list:    $list"
    if ($programs) {
        Write-Info "    suspend: $(($programs.suspend | ForEach-Object { $_.exe }) -join ', ')"
        Write-Info "    stop:    $(if ($programs.stop.Count) { ($programs.stop | ForEach-Object { $_.exe }) -join ', ' } else { '(none)' })"
        Write-Info "    trigger: $(if ($programs.trigger) { $programs.trigger.exe } else { '(none)' }); large: $(if ($programs.large) { $programs.large.exe } else { '(none)' }); elevated: $(if ($programs.elevated) { $programs.elevated.exe } else { '(none)' })"
    }
    if ($running.Count) { Write-Note "  running kaltstellen processes: $(($running | ForEach-Object { "$($_.Id) $($_.Path)" }) -join '; ')" }
    if ($DryRun) { Write-Info ''; Write-Info '[dry run] nothing was changed.'; return }

    if ($running.Count -and -not $Sandbox) {
        Write-Note 'kaltstellen runs already. Its configuration is about to be replaced, so it has to quit first.'
        Write-Note 'Quit resumes frozen programs; stopped programs stay stopped (switch to Neutral first if they should come back).'
        if (-not (Confirm-Yes 'Quit the running kaltstellen now?')) { throw 'setup cancelled: kaltstellen is still running' }
        foreach ($w in @([KsBeta.Native]::Windows() | Where-Object { $_.Class -eq 'kaltstellen-tray' -or $_.Class -eq 'kaltstellen-main' })) {
            if ($running.Id -contains [int]$w.Pid) { $null = [KsBeta.Native]::PostMessage($w.Handle, $script:WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero) }
        }
        $left = Wait-Until 30 { -not (Get-RunningKaltstellen).Count }
        if (-not $left) { throw 'kaltstellen did not quit; quit it from its menu and run setup again' }
    }

    New-Item -ItemType Directory -Force -Path $sessionDir, $tempRoot | Out-Null
    $envMap = [ordered]@{}
    if ($Sandbox) {
        $envMap.KALTSTELLEN_CONFIG_DIR = $configDir
        $envMap.KALTSTELLEN_DATA_DIR = $dataDir
        $envMap.KALTSTELLEN_RUN_KEY = $runKey
    }
    if ($script:IsUnattended) {
        $envMap.KALTSTELLEN_AUTOSTART_PROMPT = 'yes'
        $envMap.KALTSTELLEN_UPDATE = 'off'
        $envMap.KALTSTELLEN_NOTIFY = 'log'
    }
    $script:State = [pscustomobject]@{
        format = $script:StateFormat; phase = 'setup'; stamp = $stamp; setup_utc = (Get-UtcStamp)
        sandbox = [bool]$Sandbox; unattended = [bool]$script:IsUnattended
        session_dir = $sessionDir; temp_root = $tempRoot; test_dir = (Join-Path $tempRoot 'test')
        registry_root = $regRoot; config_dir = $configDir; data_dir = $dataDir
        exe = (Join-Path $install 'kaltstellen.exe'); install_dir = $install
        install = $null; backup = $null; env = [pscustomobject]$envMap; programs_file = $null
        started = @(); helper_pids = @(); pending = $null; version = $null
    }
    New-Item -ItemType Directory -Force -Path $script:BackupRoot | Out-Null
    Save-State
    $script:Session = $sessionDir; $script:KsExe = $script:State.exe
    $script:ConfigDir = $configDir; $script:DataDir = $dataDir; $script:TestDir = $script:State.test_dir
    Set-KsEnvironment
    try {
        # 1. The executable.
        Write-Title 'Executable'
        $dl = Join-Path $sessionDir 'download'
        New-Item -ItemType Directory -Force -Path $dl | Out-Null
        if ($Exe) {
            $file = $source
            $hash = Get-Sha256 $file
            Write-Info "  local file $file"
            Write-Info "  sha256 $hash"
        } else {
            $name = "kaltstellen-$Version-windows-$arch.exe"
            $file = Join-Path $dl $name
            Write-Info "  downloading $source"
            Get-ReleaseFile $source $file
            $sums = Join-Path $dl 'checksums.txt'
            Get-ReleaseFile "https://github.com/$($script:Repo)/releases/download/v$Version/checksums.txt" $sums
            $want = $null
            foreach ($l in Get-Content -LiteralPath $sums) { if ($l -match "^([0-9a-f]{64})  $([regex]::Escape($name))$") { $want = $Matches[1] } }
            if (-not $want) { throw "checksums.txt has no line for $name" }
            $hash = Get-Sha256 $file
            if ($hash -ne $want) { throw "SHA-256 mismatch for $name`: $hash, checksums.txt says $want. Nothing was installed." }
            Write-Good "  sha256 $hash matches checksums.txt"
        }
        $installed = [ordered]@{ source = $source; sha256 = $hash; created_dir = (-not (Test-Path -LiteralPath $install)); preexisting = @() }
        New-Item -ItemType Directory -Force -Path $install | Out-Null
        $keep = Join-Path $sessionDir 'backup\install'
        foreach ($n in 'kaltstellen.exe', 'kaltstellen.old.exe') {
            $f = Join-Path $install $n
            if (Test-Path -LiteralPath $f) {
                New-Item -ItemType Directory -Force -Path $keep | Out-Null
                Copy-Item -LiteralPath $f -Destination (Join-Path $keep $n)
                $installed.preexisting += [pscustomobject]@{ name = $n; sha256 = (Get-Sha256 $f) }
            }
        }
        Set-State 'install' ([pscustomobject]$installed)
        if (-not ($file -ieq $script:KsExe)) { Copy-Item -LiteralPath $file -Destination $script:KsExe -Force }
        $v = Invoke-Ks --version
        Set-State 'version' (($v.Output | Select-Object -First 1))
        Write-Info "  installed $($script:KsExe): $($script:State.version)"
        if (-not $Exe -and $script:State.version -ne "kaltstellen $Version") { Write-Note "  expected 'kaltstellen $Version'" }

        # 2. Backup of the real configuration, data and Run value.
        if (-not $Sandbox) {
            Write-Title 'Backup'
            $b = [ordered]@{}
            $b.config = Backup-Folder $realConfig (Join-Path $sessionDir 'backup\config')
            $b.data = Backup-Folder $realData (Join-Path $sessionDir 'backup\data')
            $b.run_value = Get-RunValue $script:RealRunKey
            $b.verified = $true
            $b.time = Get-UtcStamp
            Write-Utf8 (Join-Path $sessionDir 'manifest.json') (([pscustomobject]$b) | ConvertTo-Json -Depth 6)
            Set-State 'backup' ([pscustomobject]$b)
            Write-Good "  configuration: $(if ($b.config.existed) { "$(@(Get-List $b.config.files).Count) file(s) backed up and verified" } else { 'did not exist' })"
            Write-Good "  data:          $(if ($b.data.existed) { "$(@(Get-List $b.data.files).Count) file(s) backed up and verified" } else { 'did not exist' })"
            Write-Good "  Run value:     $(if ($b.run_value.present) { $b.run_value.data } else { 'absent' })"
        }

        # 3. Program list, sample files, test configuration.
        Write-Title 'Test configuration'
        $plist = Join-Path $sessionDir 'programs.json'
        Copy-Item -LiteralPath $list -Destination $plist
        Set-State 'programs_file' $plist
        $script:Programs = $programs
        New-Item -ItemType Directory -Force -Path $script:TestDir | Out-Null
        New-SampleFiles $script:TestDir
        if ($Sandbox) { New-Item -ItemType Directory -Force -Path $configDir, $dataDir | Out-Null }
        else {
            if (Test-Path -LiteralPath $configDir) { Clear-Folder $configDir } else { New-Item -ItemType Directory -Path $configDir | Out-Null }
        }
        Write-Utf8 (Join-Path $configDir 'config.toml') (New-TestConfig $programs)
        # Automatic switching starts paused; test 'triggers' switches it on.
        Set-SettingsValue 'triggers_paused' $true
        $cc = Invoke-Ks check-config
        $cc.Output | ForEach-Object { Write-Info "  $_" }
        if ($cc.Code -ne 0) { throw "the generated configuration is not valid (exit code $($cc.Code)); check the program list" }

        # 4. The tray.
        Write-Title 'Tray'
        if (-not $script:IsUnattended) {
            Write-Note '  The tray asks once whether kaltstellen should start with Windows: answer Yes (test "signin").'
            if ($Sandbox) { Write-Note '  (sandbox: the answer goes to a test registry key that Windows never runs)' }
        }
        $t = Start-Tray
        Write-Good "  tray running (pid $($t.Pid))"
        Set-State 'phase' 'ready'
    } catch {
        Write-Bad "setup failed: $($_.Exception.Message)"
        Write-Bad "Run 'beta-test.ps1 teardown' to undo what setup did."
        throw
    }
    Write-Title 'Ready'
    Write-Info "  Next: .\scripts\beta-test.ps1 run list"
    Write-Info "  Way out at any time: $(Get-WayOutCommand)"
    if ($Sandbox) {
        Write-Info '  For kaltstellen commands of your own in another terminal, set first:'
        Write-Info "    `$env:KALTSTELLEN_CONFIG_DIR='$configDir'; `$env:KALTSTELLEN_DATA_DIR='$dataDir'; `$env:KALTSTELLEN_RUN_KEY='$runKey'"
    }
}

# --- Tests --------------------------------------------------------------------

function Get-First { $script:Programs.suspend[0] }

function Test-Tray {
    $null = Start-Tray
    Set-Neutral
    Invoke-Step tray icon 'Icon, tooltip and menu (manual-checks "Tray icon and menu" 1-3)' -Human -Say @(
        'Find the kaltstellen icon in the notification area (maybe in the overflow area): a blue tile, sharp at your scaling and on a second monitor.',
        'Hover: the tooltip reads "kaltstellen: neutral".',
        'Right click and left click: the menu lists the beta-* profiles, Neutral (checked), Start at sign-in, Open kaltstellen..., Quit; clicking elsewhere closes it.'
    ) -Check {
        param($since)
        $t = Get-MainTray
        $a = Get-Active
        @{ ok = ([bool]$t -and $a -eq 'neutral'); m = [ordered]@{ tray_pid = $(if ($t) { $t.Pid }); active = $a } }
    } -Ask 'Icon, tooltip and menu as described?'

    $null = Request-Running $script:Programs.suspend
    Invoke-Step tray profile 'Choose a profile from the menu (step 4)' -Freeze -Human -Robot { $null = Invoke-Ks switch --no-wait beta-freeze } -Say @(
        'Choose "beta-freeze" in the tray menu.',
        'Expect: the icon turns orange, the tooltip names the profile, the profile is checked.'
    ) -Check {
        param($since)
        $line = Wait-Log '(menu asks for|command line asks for switch to) "beta-freeze"' $since 10 -Regex
        $null = Wait-Log 'switch to "beta-freeze" finished' $since 30
        $f = Get-FreezeReport $script:Programs.suspend
        @{ ok = ([bool]$line -and (Get-Active) -eq 'beta-freeze' -and $f.all); m = [ordered]@{ menu_line = [bool]$line; frozen = $f.detail } }
    } -Ask 'Icon orange, tooltip names beta-freeze, the profile checked?'
    Set-Neutral

    $cfg = Join-Path $script:ConfigDir 'config.toml'
    $original = [IO.File]::ReadAllText($cfg)
    try {
        Invoke-Step tray config-invalid 'Broken configuration (step 6)' -Prepare {
            Write-Utf8 $cfg ("beta_test_unknown_key = true`r`n" + $original)
            $cc = Invoke-Ks check-config
            [ordered]@{ check_config_exit = $cc.Code; check_config = ($cc.Output | Select-Object -First 2) -join ' | ' }
        } -Say @(
            'The script added an unknown key to the configuration. Open the tray menu:',
            'the icon turns red and the menu shows "Configuration invalid: ...".'
        ) -Check {
            param($since)
            $cc = Invoke-Ks check-config
            @{ ok = ($cc.Code -ne 0) }
        } -Ask 'Red icon and "Configuration invalid" in the menu?' -Human -Robot {}
    } finally { Write-Utf8 $cfg $original }
    Invoke-Step tray config-fixed 'Configuration fixed again' -Human -Robot {} -Say @('The file is fixed again. Open the menu: the error is gone, the icon is blue.') -Check {
        param($since)
        $cc = Invoke-Ks check-config
        @{ ok = ($cc.Code -eq 0) }
    } -Ask 'Error gone after opening the menu?'

    Invoke-Step tray explorer 'Explorer restart (step 7)' -Human -Prepare {
        $script:Ctx.explorer = @(Get-Process explorer -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $script:SessionId } | ForEach-Object { $_.Id })
        $null
    } -Say @('Task Manager, Processes, "Windows Explorer", Restart. Wait until the taskbar is back.') -Check {
        param($since)
        $now = @(Get-Process explorer -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $script:SessionId } | ForEach-Object { $_.Id })
        $restarted = @($now | Where-Object { $script:Ctx.explorer -notcontains $_ }).Count -gt 0
        @{ ok = [bool](Get-MainTray); m = [ordered]@{ explorer_restarted = $restarted; tray_alive = [bool](Get-MainTray) } }
    } -Ask 'The kaltstellen icon came back by itself?'

    if ($script:State.sandbox) {
        Add-Skip tray shortcut 'sandbox: a start from Explorer would not use the test configuration' 'n/a'
    } else {
        Invoke-Step tray shortcut 'Start from Explorer (step 8)' -Human -Prepare {
            Stop-Tray
            Start-Process explorer.exe -ArgumentList (Quote-Arg $script:TestDir)
            $null
        } -Say @("The tray was quit. In the Explorer window ($($script:TestDir)) double-click the shortcut 'kaltstellen tray'.",
            'A console window may flash and must close by itself; the icon appears.') -Check {
            param($since)
            $t = Wait-Until 20 { Get-MainTray }
            $released = Wait-Log 'console released' $since 5
            @{ ok = [bool]$t; m = [ordered]@{ tray_pid = $(if ($t) { $t.Pid }); console_released = [bool]$released } }
        } -Ask 'The console window closed by itself and the icon appeared?'
    }
    $null = Start-Tray

    Invoke-Step tray quit 'Quit (step 9)' -Human -Robot { $t = Get-MainTray; $null = [KsBeta.Native]::PostMessage($t.Handle, $script:WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero) } -Say @('Choose Quit in the tray menu: the icon disappears.') -Check {
        param($since)
        $gone = Wait-Until 20 { -not (Get-MainTray) }
        $line = Wait-Log 'tray: quit requested' $since 3
        @{ ok = ([bool]$gone -and [bool]$line) }
    } -Ask 'The icon disappeared?'
    $null = Start-Tray
}

function Test-Signin {
    $null = Start-Tray
    $exe = $script:KsExe
    Invoke-Step signin question 'Start at sign-in question (L8, manual-checks "Start at sign-in question")' -Say @(
        'At the first start the tray asked whether kaltstellen should start with Windows (in setup).',
        'If the question is still open: answer Yes. If you answered No: choose "Start at sign-in" in the tray menu now.'
    ) -Check {
        param($since)
        $asked = Find-Log 'asking whether to start at sign-in' $script:State.setup_utc
        $a = Invoke-Ks autostart
        $rv = Get-RunValue (Get-RunKey)
        $expected = "`"$exe`" tray"
        @{ ok = ($a.Text -match '\bon\b' -and $rv.present -and $rv.data -ieq $expected)
           m = [ordered]@{ asked_in_log = [bool]$asked; autostart = $a.Text; run_key = "HKCU\$(Get-RunKey)"; run_value = $rv.data } }
    } -Ask 'The tray icon and its menu worked while the question was open?'
    if ($script:State.sandbox) {
        Add-Skip signin at-sign-in 'sandbox: Windows never runs the test Run key' 'n/a'
    } else {
        Add-Skip signin at-sign-in 'checked after the next sign-in by "collect" (test t7)' 'skip'
    }
}

function Get-GuiWindows {
    $ours = @(Get-OurProcesses | ForEach-Object { $_.Id })
    if (-not $ours.Count) { return @() }
    @(Get-WindowsOf $ours 'kaltstellen-main' | Where-Object { $_.Visible })
}

function Open-Gui {
    $w = Get-GuiWindows
    if ($w.Count) { return [ordered]@{ already_open = $true } }
    $since = Get-UtcStamp (-200)
    Assert-SandboxEnv
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $script:KsExe -ArgumentList 'gui' -PassThru
    $win = Wait-Until 20 { (Get-GuiWindows).Count } 50
    $winMs = $sw.ElapsedMilliseconds
    $ready = Wait-Log 'gui: page ready' $since 15
    [ordered]@{ gui_pid = $p.Id; window_ms = $(if ($win) { $winMs } else { -1 }); page_ready_line = $(if ($ready) { $ready.Substring(24).Trim() }) }
}

function Close-Gui {
    foreach ($w in Get-GuiWindows) { $null = [KsBeta.Native]::PostMessage($w.Handle, $script:WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero) }
    $null = Wait-Until 15 { -not (Get-GuiWindows).Count }
}

function Get-WebViewProcesses {
    $marker = (Join-Path $script:DataDir 'webview2')
    @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue | Where-Object { $_.CommandLine -and $_.CommandLine.IndexOf($marker, [StringComparison]::OrdinalIgnoreCase) -ge 0 } | ForEach-Object { [int]$_.ProcessId })
}

function Test-Window {
    $null = Start-Tray
    Set-Neutral
    Close-Gui
    Invoke-Step window open 'Open the main window (manual-checks "Main window" 1-3)' -Prepare { Open-Gui } -Say @(
        'The window "kaltstellen" opened. Check it at 100 %, 150 % and 200 % scaling and on a monitor with another scaling: sharp, nothing cut off, no horizontal scroll bar.',
        'Switch Windows between light and dark mode: the window follows.',
        'Keyboard only: Tab reaches everything in a sensible order, the focus ring is visible, Enter and Space press buttons, Escape closes a dialog.'
    ) -Check {
        param($since)
        @{ ok = ((Get-GuiWindows).Count -eq 1) }
    } -Ask 'Look, scaling, dark mode and keyboard as described?' -AskNote

    $cfg = Join-Path $script:ConfigDir 'config.toml'
    Invoke-Step window edit 'Editing (steps 4-7, 9, 12)' -Human -Robot {} -Say @(
        'Profiles: New, Rename, Copy, Delete change the list; an invalid name shows the reason (use new profiles, not the beta-* ones).',
        'Hotkey: type Ctrl+Alt+Banana, Set: marked and explained; a valid one is saved.',
        'Edit on an entry: the options change with the action; Start without a path is refused.',
        'Add program...: running programs with memory and protection; protected ones cannot be added.',
        'Edit config.toml in an editor while the window is open, then change something in the window: the status line says the file changed and reloads it.'
    ) -Check {
        param($since)
        $text = [IO.File]::ReadAllText($cfg)
        $cc = Invoke-Ks check-config
        @{ ok = ($text.Contains('# beta-test: generated') -and $cc.Code -eq 0); m = [ordered]@{ comments_kept = $text.Contains('# beta-test: generated'); check_config_exit = $cc.Code } }
    } -Ask 'Editing works as described?' -AskNote

    $null = Request-Running $script:Programs.suspend
    Invoke-Step window apply 'Apply profile (step 8)' -Freeze -Human -Robot { $null = Invoke-Ks switch beta-freeze } -Say @(
        'Select "beta-freeze" in the window and press "Apply profile": the button shows "Applying...", then the status line the result; the tray icon turns orange.'
    ) -Check {
        param($since)
        $line = Wait-Log 'switch to "beta-freeze"' $since 10
        $null = Wait-Log 'switch to "beta-freeze" finished' $since 30
        $f = Get-FreezeReport $script:Programs.suspend
        @{ ok = ([bool]$line -and $f.all); m = [ordered]@{ frozen = $f.detail } }
    } -Ask 'Button, status line and tray icon as described?'
    Set-Neutral

    Invoke-Step window misc 'Start at sign-in, context menu, updates section (steps 11, 13, 13a)' -Human -Say @(
        '"Start at sign-in" in the header follows the tray menu item.',
        'Right click in the page: no browser context menu; Ctrl+P, F5, Ctrl+F do nothing.',
        'Updates (sidebar): the installed version is shown. "Check now" contacts GitHub (your choice): "Checking...", then the result.'
    ) -Ask 'All as described?' -AskNote

    Invoke-Step window icons 'Program icons (13b)' -Human -Robot {} -Say @(
        'Next to each program in a profile, in "Add program" and in the unassigned list its icon appears, sharp at every scaling; programs without an icon get a grey square; rows do not move.'
    ) -Check {
        param($since)
        $lines = Find-Log 'gui: \d+ icons at' $script:State.setup_utc -Regex -All
        @{ ok = $null; m = [ordered]@{ icon_requests = $lines.Count; last = $(if ($lines.Count) { $lines[-1].Substring(24).Trim() }) } }
    } -Ask 'Icons as described?'

    Invoke-Step window single 'One window only (step 14)' -Human -Robot {} -Say @('Minimise the window, then press Enter: the script runs "kaltstellen gui" again.') -Check {
        param($since)
        $p = Start-Process -FilePath $script:KsExe -ArgumentList 'gui' -PassThru
        $exited = $p.WaitForExit(10000)
        Start-Sleep -Milliseconds 800
        $count = (Get-GuiWindows).Count
        @{ ok = ($exited -and $count -eq 1); m = [ordered]@{ second_process_exited = $exited; windows = $count } }
    } -Ask 'The same window came back to the front, no second one?'

    Invoke-Step window close 'Close ends its processes (step 15)' -Prepare {
        $script:Ctx.webview = Get-WebViewProcesses
        [ordered]@{ webview_processes = $script:Ctx.webview.Count }
    } -Say @('Close the window.') -Check {
        param($since)
        if ($script:IsUnattended) { Close-Gui }
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $closed = Wait-Until 15 { -not (Get-GuiWindows).Count }
        $gone = Wait-Until 15 { -not @($script:Ctx.webview | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }).Count }
        @{ ok = ([bool]$closed -and [bool]$gone); m = [ordered]@{ all_ended_ms = $sw.ElapsedMilliseconds } }
    }
    Invoke-Step window from-tray '"Open kaltstellen..." from the tray (step 15)' -Human -Say @('Choose "Open kaltstellen..." in the tray menu: the window opens without a console window. Close it again.') -Check {
        param($since)
        $line = Find-Log 'tray: main window started' $since
        @{ ok = [bool]$line }
    } -Ask 'Opened without a console window?'
    Close-Gui
}

function Test-Failure {
    $null = Start-Tray
    Set-Neutral
    $first = Get-First
    $missing = Join-Path (Join-Path $script:TestDir 'missing') 'ks-beta-missing.exe'
    if (Test-Path -LiteralPath $missing) {
        Get-ProgramProcesses 'ks-beta-missing.exe' | Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 300
        Remove-Item -LiteralPath $missing -Force
    }
    $null = Request-Running @($first)
    $null = Open-Gui
    Invoke-Step failure apply 'A switch that fails (manual-checks 13c, tray step 5)' -Freeze -Prepare {
        $r = Switch-Profile 'beta-failure'
        $script:Ctx.failure = $r
        [ordered]@{ exit = $r.Code; output = ($r.Output -join ' | ') }
    } -Say @(
        'The script applied "beta-failure": its start entry points at a program that does not exist.',
        'Tray: the icon is red with an exclamation mark, the menu shows the error line; opening and closing the menu changes nothing.',
        'Window, profile beta-failure: the start entry has a red mark "KS110 start failed", hovering shows the whole error, the row has a red edge; other entries and profiles show nothing.'
    ) -Check {
        param($since)
        $r = $script:Ctx.failure
        $f = Get-FreezeReport @($first)
        @{ ok = ($r.Code -eq 1 -and $r.Text -match 'KS110' -and $f.all); m = [ordered]@{ frozen = $f.detail } }
    } -Ask 'Red icon, error line in the menu, marker in the window as described?' -AskNote

    $resolve = [pscustomobject]@{ exe = 'ks-beta-missing.exe'; path = $missing; args = @('-n', '600', '127.0.0.1'); disposable = $true; hidden = $true }
    Invoke-Step failure resolves 'The error resolves by itself' -Prepare {
        Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\PING.EXE') -Destination $missing
        $script:Ctx.resolve_since = Get-UtcStamp (-200)
        $null = Start-Program $resolve -Hidden
        [ordered]@{ started = 'ks-beta-missing.exe (a copy of ping.exe) at the missing path' }
    } -Say @(
        'The script put a program at that path and started it.',
        'Within about 30 s (or when the menu opens) the tray icon turns orange again without a switch.',
        'Click into another window and back into kaltstellen: the marker is gone.'
    ) -Check {
        param($since)
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $line = Wait-Log 'tray: error resolved by itself' $script:Ctx.resolve_since 45
        @{ ok = [bool]$line; m = [ordered]@{ resolved_line = $(if ($line) { $line.Substring(24).Trim() }); waited_ms = $sw.ElapsedMilliseconds } }
    } -Ask 'Icon orange again and the marker gone?'

    Invoke-Step failure clean 'A clean switch removes all markers' -Prepare {
        $r = Switch-Profile neutral
        [ordered]@{ neutral_exit = $r.Code }
    } -Say @('The script switched to neutral. Look at the window (light and dark mode): no markers left.') -Check {
        param($since)
        $f = Get-FreezeReport @($first)
        @{ ok = ((Get-Active) -eq 'neutral' -and $f.none) }
    } -Ask 'No markers left?'
    $null = Close-Program $resolve -Wait
    Close-Gui
}

function Test-DragDrop {
    $null = Start-Tray
    $drop = Join-Path $script:TestDir 'dragdrop'
    Invoke-Step dragdrop drop 'Drag and drop from Explorer (manual-checks 13d)' -Human -Prepare {
        $o = Open-Gui
        Start-Process explorer.exe -ArgumentList (Quote-Arg $drop)
        $o
    } -Say @(
        "Explorer shows $drop. In the window open the profile 'beta-dragdrop'.",
        '1. Drag ks-beta-drop1.exe onto the profile: while dragging the table has a dashed outline; on drop it is added.',
        '2. Drag anydesk.exe (a renamed copy of ping.exe, on the warn list): a question appears; "Add anyway" adds it with "confirmed".',
        '3. Drag C:\Windows\explorer.exe from another Explorer window: refused with the reason in the status line. Drag ks-beta-drop1.exe again: refused as a duplicate.',
        '4. Select ks-beta-drop2.exe, ks-beta-drop3.exe, notes.txt, "shortcut to drop1.lnk" and "a folder" and drop them together:',
        '   the status line says "2 of 5 added ... Not added: ..." and names each refused item; click elsewhere and back: the summary stays.',
        '5. Drop onto the sidebar or the header: nothing happens. Drop a link or text from a browser: nothing happens.',
        '6. Optional: an .exe from a network share or a USB stick is refused as not on a local disk.'
    ) -Check {
        param($since)
        $text = [IO.File]::ReadAllText((Join-Path $script:ConfigDir 'config.toml'))
        $section = ''
        if ($text -match '(?s)name = "beta-dragdrop"(.*?)(\r?\n# |\z)') { $section = $Matches[1] }
        $has = { param($n) $section -match ('exe = "' + [regex]::Escape($n) + '"') }
        $m = [ordered]@{}
        foreach ($n in 'ks-beta-drop1.exe', 'ks-beta-drop2.exe', 'ks-beta-drop3.exe', 'anydesk.exe', 'explorer.exe', 'notes.txt') { $m[$n] = [bool](& $has $n) }
        $m['anydesk_confirmed'] = [bool]($section -match '(?s)exe = "anydesk\.exe".*?confirmed = true')
        $ok = $m['ks-beta-drop1.exe'] -and $m['ks-beta-drop2.exe'] -and $m['ks-beta-drop3.exe'] -and -not $m['explorer.exe'] -and -not $m['notes.txt']
        @{ ok = $ok; m = $m }
    } -Ask 'Outline, question, refusals and status line as described?' -AskNote
    Close-Gui
}

function Test-Descriptions {
    $null = Start-Tray
    $running = Request-Running $script:Programs.suspend
    Invoke-Step descriptions show 'Program descriptions (beta-test 1.4)' -Prepare {
        $m = [ordered]@{}
        foreach ($p in $running) { $m["list $($p.exe)"] = ((Invoke-Ks list ([IO.Path]::GetFileNameWithoutExtension($p.exe))).Output | Select-Object -Skip 1 -First 3) -join ' | ' }
        $m['status --unassigned'] = ((Invoke-Ks status --unassigned).Output | Select-Object -First 12) -join ' | '
        $null = Open-Gui
        $m
    } -Say @(
        'In the window: the description, product and publisher of your programs (profile beta-freeze, "Add program...", the unassigned list).',
        'Also look at the output of "kaltstellen list" and "kaltstellen status --unassigned" above (recorded in the results).'
    ) -Ask 'Descriptions, products and publishers shown and right?' -AskNote
    Close-Gui
}

function Start-HelperTray([string]$Hotkey) {
    $dir = Join-Path $script:State.temp_root 'helper'
    New-Item -ItemType Directory -Force -Path (Join-Path $dir 'config'), (Join-Path $dir 'data') | Out-Null
    Write-Utf8 (Join-Path $dir 'config\config.toml') "# Second tray of the beta test that holds the hotkey first.`r`n[[profile]]`r`nname = `"beta-helper`"`r`nhotkey = $(ConvertTo-TomlString $Hotkey)`r`n"
    $saved = @{}
    foreach ($n in $script:EnvNames) { $saved[$n] = [Environment]::GetEnvironmentVariable($n, 'Process') }
    try {
        $env:KALTSTELLEN_CONFIG_DIR = Join-Path $dir 'config'
        $env:KALTSTELLEN_DATA_DIR = Join-Path $dir 'data'
        $env:KALTSTELLEN_RUN_KEY = "$($script:State.registry_root)\HelperRun"
        $env:KALTSTELLEN_AUTOSTART_PROMPT = 'skip'
        $env:KALTSTELLEN_UPDATE = 'off'
        $p = Start-Process -FilePath $script:KsExe -ArgumentList 'tray' -WindowStyle Hidden -PassThru
    } finally {
        foreach ($n in $script:EnvNames) { [Environment]::SetEnvironmentVariable($n, $saved[$n], 'Process') }
    }
    Set-State 'helper_pids' (@(Get-HelperPids) + @($p.Id))
    $null = Wait-Until 15 { @(Get-TrayWindows | Where-Object { [int]$_.Pid -eq $p.Id }).Count }
    Start-Sleep -Milliseconds 500
    $p
}

function Stop-HelperTrays {
    foreach ($id in Get-HelperPids) {
        $w = Get-TrayWindows | Where-Object { [int]$_.Pid -eq $id } | Select-Object -First 1
        if ($w) { $null = Stop-TrayWindow $w 10 } else { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
    }
    Set-State 'helper_pids' @()
}

# Unattended: one short press, then one held down (the key repeats).
function Send-Hotkey([string]$Hotkey) {
    $vk = ConvertTo-VirtualKeys $Hotkey
    if (-not $vk) { throw "cannot type $Hotkey" }
    $null = [KsBeta.Keys]::Chord($vk.mods, $vk.key, 1)
    Start-Sleep -Milliseconds 700
    $null = [KsBeta.Keys]::Chord($vk.mods, $vk.key, 15)
    Start-Sleep -Milliseconds 700
}

function Test-Hotkeys {
    $hk = $script:Programs.hotkey
    $null = Start-Tray
    Set-Neutral
    $null = Request-Running $script:Programs.suspend
    Invoke-Step hotkeys press "A real key press (manual-checks ""Hotkeys"" 1, T17)" -Freeze -Human -Robot { Send-Hotkey $hk } -Say @(
        "Press $hk once briefly. Then press it a second time and hold it for 2 s.",
        'Expect: "beta-freeze" is applied (icon orange), once per press even when held down.'
    ) -Check {
        param($since)
        Start-Sleep -Milliseconds 800
        $hits = Find-Log 'hotkey asks for "beta-freeze"' $since -All
        $f = Get-FreezeReport $script:Programs.suspend
        @{ ok = ($hits.Count -eq 2 -and (Get-Active) -eq 'beta-freeze'); m = [ordered]@{ hotkey_requests = $hits.Count; frozen = $f.detail } }
    } -Ask 'The icon turned orange on the first press?'
    Set-Neutral

    Invoke-Step hotkeys taken 'A hotkey that is taken (step 2)' -Human -Robot {} -Prepare {
        Stop-Tray
        $h = Start-HelperTray $hk
        $script:Ctx.tray_since = Get-UtcStamp (-200)
        $null = Start-Tray
        [ordered]@{ helper_tray_pid = $h.Id }
    } -Say @(
        "A second test tray holds $hk now, then the script started your tray again.",
        "Open the tray menu: it lists ""Hotkey not available: $hk ..."". Choose ""beta-freeze"" from the menu: it still works. Then choose Neutral."
    ) -Check {
        param($since)
        $line = Find-Log "is not available" $script:Ctx.tray_since
        @{ ok = [bool]$line; m = [ordered]@{ log = $(if ($line) { $line.Substring(24).Trim() }) } }
    } -Ask 'The menu lists "Hotkey not available" and the profile works from the menu?'
    Set-Neutral
    Stop-HelperTrays
    Stop-Tray
    $traySince = Get-UtcStamp (-200)
    $null = Start-Tray

    Invoke-Step hotkeys altgr "Ctrl+Alt with a digit (step 3)" -Say @(
        "beta-altgr has the hotkey $($script:Programs.altgr_hotkey); the tray refuses it.",
        'With a German keyboard layout: type AltGr+7 in an editor: "{" must appear. Without a German layout answer s.'
    ) -Check {
        param($since)
        $line = Find-Log ([regex]::Escape($script:Programs.altgr_hotkey)) $traySince -Regex
        @{ ok = [bool]$line; m = [ordered]@{ log = $(if ($line) { $line.Substring(24).Trim() }) } }
    } -Ask 'AltGr+7 still types "{"?'
}

function Test-Unassigned {
    $tp = $script:Programs.tray_program
    $null = Start-Tray
    Set-Neutral
    if ($tp -and (Get-ProgramProcesses $tp.exe).Count) {
        Write-Note "   $($tp.exe) runs already; T22 needs it to start after the profile."
        $null = Request-Closed $tp
    }
    $null = Request-Running $script:Programs.suspend
    Invoke-Step unassigned listed 'A new program with a tray icon (T22)' -Freeze -Prepare {
        $r = Switch-Profile 'beta-freeze'
        Start-Sleep -Seconds 2
        if ($tp -and $tp.path) { $null = Start-Program $tp; Start-Sleep -Seconds 8 }
        [ordered]@{ switch_exit = $r.Code }
    } -Say @(
        $(if ($tp -and $tp.path) { "The script applied beta-freeze and started $($tp.exe)." } else { 'The script applied beta-freeze. Now start a program with a tray icon that is in no profile (a chat or sync tool) and wait a few seconds.' }),
        'Tray menu: "Unassigned programs (1)" (or more) appears and opens the main window.',
        'Window, profile beta-freeze: the program is listed as "new". "Add to profile" adds it and it leaves the list; "Dismiss" removes another one from this profile''s list only.'
    ) -Check {
        param($since)
        $u = Invoke-Ks status --unassigned
        $ok = $null
        if ($tp) { $ok = [bool]($u.Output | Where-Object { $_ -match [regex]::Escape($tp.exe) -and $_ -match '\(new\)' }) }
        @{ ok = $ok; m = [ordered]@{ unassigned = (($u.Output | Select-Object -First 10) -join ' | ') } }
    } -Ask 'Menu item, "new" marker, "Add to profile" and "Dismiss" as described?' -AskNote
    Set-Neutral
    Close-Gui
}

function Test-Stop {
    if (-not $script:Programs.stop.Count) { Add-Skip stop closes 'no program in the "stop" list'; return }
    $null = Start-Tray
    Set-Neutral
    $running = Request-Running $script:Programs.stop 3000
    Invoke-Step stop closes 'A stop entry closes the program' -Freeze -Prepare {
        $r = Switch-Profile 'beta-stop'
        [ordered]@{ exit = $r.Code; ms = $r.Ms; output = ($r.Output -join ' | ') }
    } -Check {
        param($since)
        $m = [ordered]@{}
        $ok = $true
        foreach ($p in $running) { $alive = (Get-ProgramProcesses $p.exe).Count; $m[$p.exe] = $(if ($alive) { 'still running' } else { 'closed' }); if ($alive -and $p.on_timeout -ne 'keep') { $ok = $false } }
        $m.status = ((Invoke-Ks status).Output -join ' | ')
        $stayed = @($m.Values | Where-Object { $_ -eq 'still running' }).Count
        @{ ok = $ok; m = $m; note = $(if ($stayed) { 'with on_timeout = "keep" a program that asks to save stays open and is reported (KS108)' }) }
    } -Ask 'The programs closed (or asked to save and stayed, reported)?'
    Invoke-Step stop restart 'Neutral starts them again (restart_on_neutral)' -Prepare {
        $r = Switch-Profile neutral
        [ordered]@{ exit = $r.Code; ms = $r.Ms }
    } -Check {
        param($since)
        Start-Sleep -Seconds 3
        $m = [ordered]@{}; $ok = $true
        foreach ($p in $running) { $alive = (Get-ProgramProcesses $p.exe).Count -gt 0; $m[$p.exe] = $alive; if ($p.restart -and -not $alive) { $ok = $false } }
        @{ ok = $ok; m = $m }
    } -Ask 'The programs came back and work?'
}

function Test-Triggers {
    $tr = $script:Programs.trigger
    if (-not $tr) { Add-Skip triggers all 'no trigger program in beta-test.json'; return }
    $first = Get-First
    Invoke-Step triggers check-config 'check-config names the trigger and refuses it as a frozen entry (step 1)' -Check {
        param($since)
        $cc = Invoke-Ks check-config
        $listed = [bool]($cc.Output | Where-Object { $_ -match 'beta-trigger' -and $_ -match ('trigger ' + [regex]::Escape($tr.exe) + ' \(priority 0\)') })
        $bad = Join-Path $script:TestDir 'trigger-selftest.toml'
        Write-Utf8 $bad (New-TestConfig $script:Programs -TriggerSelfTest)
        $cb = Invoke-Ks check-config $bad
        Remove-Item -LiteralPath $bad -Force
        $refused = $cb.Code -ne 0 -and $cb.Text -match 'is a program that this profile freezes'
        @{ ok = ($listed -and $refused); m = [ordered]@{ listed = $listed; refused = $refused; refusal = (($cb.Output | Where-Object { $_ -match 'freezes' }) | Select-Object -First 1) } }
    }
    $null = Request-Closed $tr
    Set-Neutral
    Set-TriggersPaused $false
    $null = Request-Running @($first)
    Invoke-Step triggers menu 'Automatic switching in the menu (step 2)' -Human -Say @('Open the tray menu: "Automatic switching" is there and checked.') -Ask 'Checked?'

    $startTrigger = {
        if ($tr.path) { $script:Ctx.trigger_started = Get-UtcStamp; $null = Start-Program $tr; return [ordered]@{ started_by = 'script' } }
        Write-Info "   Start $($tr.exe) now."
        [ordered]@{ started_by = 'person' }
    }
    # The tray looks for triggers every 5 s: after closing the trigger, wait
    # until it has seen the end before starting it again.
    $closeTrigger = {
        $null = Request-Closed $tr
        Start-Sleep -Milliseconds 6500
    }
    Invoke-Step triggers applies 'The trigger applies its profile (step 3)' -Freeze -Prepare $startTrigger -Say @(
        "Within about 5 s after $($tr.exe) started: icon orange, tooltip ends in ""(by trigger)"", a notification ""kaltstellen: beta-trigger"" appears;",
        "the menu shows ""Applied by trigger $($tr.exe); back to neutral when it ends"" and starts with ""Back to neutral"". Do not click the notification yet."
    ) -Check {
        param($since)
        $ask = Wait-Log 'trigger asks for "beta-trigger"' $since 20
        $note = Wait-Log 'tray: notification: kaltstellen: beta-trigger' $since 15
        $null = Wait-Log 'switch to "beta-trigger" finished' $since 20
        $ms = $null
        if ($ask -and $script:Ctx.trigger_started) { $ms = [int]((Get-LogTime $ask) - (ConvertFrom-UtcStamp $script:Ctx.trigger_started)).TotalMilliseconds }
        $f = Get-FreezeReport @($first)
        @{ ok = ([bool]$ask -and [bool]$note -and (Get-Active) -eq 'beta-trigger' -and $f.all); m = [ordered]@{ applied_after_ms = $ms; notification = $(if ($note) { $note.Substring(24).Trim() }); frozen = $f.detail } }
    } -Ask 'Icon, tooltip, notification and menu as described?' -AskNote
    $script:Ctx.trigger_started = $null

    Invoke-Step triggers click 'Click the notification (step 4)' -Human -Say @(
        'Click the notification while it is on screen (if it is gone already: choose "Back to neutral" in the menu).',
        'Expect: neutral is applied; the menu shows "Triggers wait after a manual switch ..." and no "Back to"; nothing switches back while the trigger runs.'
    ) -Check {
        param($since)
        $back = Wait-Log 'notification clicked|back to "neutral"' $since 10 -Regex
        $null = Wait-Log 'switch to "neutral" finished' $since 20
        Start-Sleep -Seconds 6
        @{ ok = ([bool]$back -and (Get-Active) -eq 'neutral'); m = [ordered]@{ how = $(if ($back) { $back.Substring(24).Trim() }) } }
    } -Ask 'Neutral, the menu as described, and no switch back while the trigger runs?'

    Invoke-Step triggers two 'Two notifications in quick succession (step 4a)' -Freeze -Prepare {
        Set-Neutral
        & $closeTrigger
        if ($tr.path) {
            $s = Get-UtcStamp
            $null = Start-Program $tr
            $null = Wait-Log 'switch to "beta-trigger" finished' $s 25
            $s = Get-UtcStamp
            $null = Request-Closed $tr
            $null = Wait-Log 'trigger asks for "neutral"' $s 25
            $null = Start-Program $tr
            return [ordered]@{ driven_by = 'script' }
        }
        [ordered]@{ driven_by = 'person' }
    } -Say @(
        $(if ($tr.path) { "The script started, closed and started $($tr.exe) again: two automatic switches follow each other." } else { "Close $($tr.exe) (the trigger waits a manual switch out first, so start and close it once), then start it, close it and start it again within about 10 s." }),
        'While both notifications are on screen a click does nothing; the menu item "Back to ..." names the profile of the latest one and works.',
        'Note the order in which the notifications appear and leave.'
    ) -Check {
        param($since)
        $null = Wait-Log 'tray: notification:' $since 20
        Start-Sleep -Seconds 2
        $n = Find-Log 'tray: notification:' $since -All
        $g = Find-Log 'notification gone|notification clicked' $since -All -Regex
        @{ ok = $null; m = [ordered]@{ notifications = @($n | ForEach-Object { $_.Substring(0, 23) + ' ' + $_.Substring(24).Trim() }); gone_or_clicked = @($g | ForEach-Object { $_.Substring(24).Trim() }) } }
    } -Ask '"Back to ..." named the latest profile and worked; a click with two on screen did nothing?' -AskNote

    Invoke-Step triggers dnd 'Do Not Disturb or a full-screen game (step 4b)' -Freeze -Human -Robot {} -Prepare {
        Set-Neutral
        & $closeTrigger
        $null
    } -Say @(
        "Turn on Do Not Disturb (or start a full-screen game). Then start $($tr.exe) (press Enter first if the script starts it: it does so after this prompt).",
        'Expect: the profile is applied, no notification appears (it goes to the notification centre), the menu item "Back to neutral" works.'
    ) -Check {
        param($since)
        $s = Get-UtcStamp
        if ($tr.path) { $null = Start-Program $tr } else { $s = $since }
        $ask = Wait-Log 'trigger asks for "beta-trigger"' $s 25
        @{ ok = [bool]$ask }
    } -Ask 'Applied without a notification on screen, and "Back to neutral" worked? (Turn Do Not Disturb off again.)'

    Invoke-Step triggers ends 'Neutral comes back when the trigger ends (step 5)' -Prepare {
        Set-Neutral
        & $closeTrigger
        $s = Get-UtcStamp
        $null = & $startTrigger
        $null = Wait-Log 'switch to "beta-trigger" finished' $s 25
        $script:Ctx.closed_at = Get-UtcStamp
        $null = Request-Closed $tr
        [ordered]@{ active_before_close = 'beta-trigger' }
    } -Check {
        param($since)
        $back = Wait-Log 'trigger asks for "neutral"' $script:Ctx.closed_at 25
        $null = Wait-Log 'switch to "neutral" finished' $script:Ctx.closed_at 25
        $ms = if ($back) { [int]((Get-LogTime $back) - (ConvertFrom-UtcStamp $script:Ctx.closed_at)).TotalMilliseconds }
        @{ ok = ([bool]$back -and (Get-Active) -eq 'neutral'); m = [ordered]@{ back_after_ms = $ms } }
    } -Ask 'A notification said so when neutral came back?'

    Invoke-Step triggers manual-wins 'A switch of your own wins (step 6)' -Freeze -Prepare {
        & $closeTrigger
        $s = Get-UtcStamp
        $null = & $startTrigger
        $null = Wait-Log 'switch to "beta-trigger" finished' $s 25
        $r = Switch-Profile 'beta-freeze'
        $script:Ctx.closed_at = Get-UtcStamp
        $null = Request-Closed $tr
        [ordered]@{ manual_switch_exit = $r.Code }
    } -Check {
        param($since)
        Start-Sleep -Seconds 15
        $a = Get-Active
        @{ ok = ($a -eq 'beta-freeze'); m = [ordered]@{ active_15s_after_trigger_ended = $a } }
    }
    Set-Neutral

    Invoke-Step triggers pause 'Pausing automatic switching (step 7)' -Human -Say @(
        'Uncheck "Automatic switching" in the tray menu.'
    ) -Check {
        param($since)
        Start-Sleep -Milliseconds 800
        $s = Read-JsonFile (Join-Path $script:ConfigDir 'settings.json')
        $paused = [bool]($s -and $s.triggers_paused)
        $null = & $startTrigger
        Start-Sleep -Seconds 12
        $a = Get-Active
        Stop-Tray
        $t0 = Get-UtcStamp (-200)
        $null = Start-Tray
        $kept = Wait-Log 'automatic switching is paused' $t0 5
        @{ ok = ($paused -and $a -eq 'neutral' -and [bool]$kept); m = [ordered]@{ settings_paused = $paused; active_with_trigger_running = $a; paused_after_restart = [bool]$kept } }
    } -Ask 'After the restart of the tray (done by the script) the item is still unchecked?'
    Invoke-Step triggers resume 'Switching it on again (step 7)' -Human -Say @('Check "Automatic switching" again: the running trigger counts as starting and the profile is applied.') -Check {
        param($since)
        $ask = Wait-Log 'trigger asks for "beta-trigger"' $since 20
        @{ ok = [bool]$ask }
    } -Ask 'The profile was applied?'
    $null = Request-Closed $tr
    Set-Neutral

    $cfg = Join-Path $script:ConfigDir 'config.toml'
    Invoke-Step triggers window 'Triggers in the main window (step 9)' -Human -Prepare { Open-Gui } -Say @(
        "Profile beta-trigger, section ""Triggers"": $($tr.exe) with priority 0. Set the priority to 5 (Set): the file keeps its comments.",
        "Enter $($first.exe) (the profile freezes it) as a trigger: refused with the reason; the text and the mark stay after clicking away and back.",
        'Enter x as the priority, Set: the message stays after clicking away and back.'
    ) -Check {
        param($since)
        $text = [IO.File]::ReadAllText($cfg)
        $p5 = [bool]($text -match '(?s)name = "beta-trigger".*?priority = 5')
        @{ ok = ($p5 -and $text.Contains('# beta-test: generated')); m = [ordered]@{ priority_5 = $p5 } }
    } -Ask 'Refusal, marks and messages as described?' -AskNote

    Invoke-Step triggers rename 'Renaming the applied profile (step 10)' -Freeze -Human -Say @(
        "Start $($tr.exe) (beta-trigger is applied), then rename beta-trigger in the window to beta-trigger2.",
        'The old name stays active until the next poll applies the renamed profile, with a notification; close the trigger: the profile from before comes back.',
        'Then rename it back to beta-trigger.'
    ) -Ask 'As described? Note what the menu and the notifications said.' -AskNote
    $null = Request-Closed $tr
    Set-Neutral
    Close-Gui
    Set-TriggersPaused $true
    Write-Info '   Step 11 (sign-out with a trigger active) is part of test t7.'
}

function Test-T13 {
    $null = Start-Tray
    Set-Neutral
    $running = Request-Running $script:Programs.suspend
    if (-not $running.Count) { Add-Skip t13 freeze 'none of the "suspend" programs runs'; return }
    $samples = New-Object System.Collections.Generic.List[object]
    Invoke-Step t13 freeze "Long freeze, $FreezeMinutes minutes (T13)" -Freeze -Prepare {
        $r = Switch-Profile 'beta-freeze'
        $script:Ctx.t13_start = Get-Date
        [ordered]@{ switch_exit = $r.Code; switch_ms = $r.Ms }
    } -Check {
        param($since)
        $end = $script:Ctx.t13_start.AddMinutes($FreezeMinutes)
        $stayed = $true
        while ((Get-Date) -lt $end) {
            $f = Get-FreezeReport $running
            $samples.Add([ordered]@{ t = [int]((Get-Date) - $script:Ctx.t13_start).TotalSeconds; all = $f.all })
            if (-not $f.all) { $stayed = $false }
            $left = [int]($end - (Get-Date)).TotalSeconds
            Write-Host ("`r   frozen for {0:n0} s, {1:n0} s to go (Ctrl+C aborts; then run recover)   " -f ((Get-Date) - $script:Ctx.t13_start).TotalSeconds, [math]::Max(0, $left)) -NoNewline
            Start-Sleep -Seconds ([math]::Min(30, [math]::Max(1, $left)))
        }
        Write-Host ''
        $f = Get-FreezeReport $running
        if (-not $f.all) { Start-Sleep -Seconds 1; $f = Get-FreezeReport $running }
        @{ ok = ($stayed -and $f.all); m = [ordered]@{ frozen_minutes = [math]::Round(((Get-Date) - $script:Ctx.t13_start).TotalMinutes, 1); samples = $samples.Count; stayed_frozen = $stayed; frozen_at_end = $f.detail }; note = $(if (-not $f.all) { 'frozen at first, not at the end: see frozen_at_end' }) }
    }
    Invoke-Step t13 resume 'Resume' -Prepare {
        $r = Switch-Profile neutral
        [ordered]@{ neutral_exit = $r.Code; neutral_ms = $r.Ms }
    } -Check {
        param($since)
        $f = Get-FreezeReport $running
        $pings = [ordered]@{}
        foreach ($p in $running) { $w = Get-MainWindows $p | Select-Object -First 1; if ($w) { $pings[$p.exe] = [KsBeta.Native]::Ping($w.Handle, 5000) } }
        @{ ok = $f.none; m = [ordered]@{ window_answer_ms = $pings } }
    }
    foreach ($p in $running) {
        Invoke-Step t13 $p.exe "After the long freeze: $($p.exe)" -Human -Say @("Check $($p.exe) by hand: network, sync, sound, licence, anything it does.") -Ask "$($p.exe) works as before the freeze?" -AskNote
    }
}

function Test-T14 {
    $null = Start-Tray
    Set-Neutral
    $running = Request-Running $script:Programs.suspend 3000
    $windowed = @($running | Where-Object { (Get-MainWindows $_).Count })
    if (-not $windowed.Count) { Add-Skip t14 shell 'none of the running "suspend" programs has a window'; return }
    $tray = [KsBeta.Native]::FindWindow('Shell_TrayWnd', $null)
    Invoke-Step t14 shell 'Shell stays responsive with windowed programs frozen (T14)' -Freeze -Prepare {
        $m = [ordered]@{ programs = @($windowed | ForEach-Object { $_.exe }); taskbar_ping_before_ms = [KsBeta.Native]::Ping($tray, 3000) }
        $r = Switch-Profile 'beta-freeze'
        $m.switch_exit = $r.Code
        Start-Sleep -Seconds 6
        $m.frozen_windows = @(foreach ($p in $windowed) { foreach ($w in Get-MainWindows $p) { [ordered]@{ exe = $p.exe; iconic = [KsBeta.Native]::IsIconic($w.Handle); hung = [KsBeta.Native]::IsHungAppWindow($w.Handle) } } })
        $m.ghost_windows = @([KsBeta.Native]::Windows() | Where-Object { $_.Class -eq 'Ghost' -and $_.Visible }).Count
        $m.taskbar_ping_frozen_ms = [KsBeta.Native]::Ping($tray, 3000)
        $m.explorer_responding = @(Get-Process explorer -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $script:SessionId } | ForEach-Object { $_.Responding })
        $folder = Join-Path $script:TestDir 'dragdrop'
        $sw = [Diagnostics.Stopwatch]::StartNew()
        Start-Process explorer.exe -ArgumentList (Quote-Arg $folder)
        $h = Wait-Until 15 { [KsBeta.Native]::Windows() | Where-Object { $_.Class -eq 'CabinetWClass' -and $_.Title -like '*dragdrop*' } | Select-Object -First 1 } 50
        $m.explorer_window_open_ms = if ($h) { $sw.ElapsedMilliseconds } else { -1 }
        if ($h) {
            Start-Sleep -Seconds 1
            $m.explorer_window_ping_ms = [KsBeta.Native]::Ping($h.Handle, 3000)
            $null = [KsBeta.Native]::PostMessage($h.Handle, $script:WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
        }
        $script:Ctx.t14 = $m
        $m
    } -Check {
        param($since)
        $m = $script:Ctx.t14
        $f = Get-FreezeReport $windowed
        $ok = $f.all -and $m.taskbar_ping_frozen_ms -ge 0 -and $m.taskbar_ping_frozen_ms -lt 1000 -and $m.explorer_window_open_ms -ge 0 -and $m.explorer_window_open_ms -lt 10000 -and $m.explorer_window_ping_ms -ge 0
        @{ ok = $ok; m = [ordered]@{ frozen = $f.detail } }
    } -Ask 'While they are frozen: do the taskbar, Start and Explorer work normally? (try them now)'
    Invoke-Step t14 resume 'Windows answer again after neutral' -Prepare {
        $r = Switch-Profile neutral
        [ordered]@{ neutral_exit = $r.Code; neutral_ms = $r.Ms }
    } -Check {
        param($since)
        Start-Sleep -Seconds 1
        $pings = [ordered]@{}; $ok = $true
        foreach ($p in $windowed) { $w = Get-MainWindows $p | Select-Object -First 1; if ($w) { $v = [KsBeta.Native]::Ping($w.Handle, 5000); $pings[$p.exe] = $v; if ($v -lt 0) { $ok = $false } } }
        @{ ok = $ok; m = [ordered]@{ window_answer_ms = $pings; ghost_windows = @([KsBeta.Native]::Windows() | Where-Object { $_.Class -eq 'Ghost' -and $_.Visible }).Count } }
    }
}

function Measure-Trim([string]$ProfileName, $Large) {
    $m = [ordered]@{ profile = $ProfileName }
    $m.ws_running_mb = Get-WorkingSetMb $Large.exe
    $m.avail_before_mb = Get-AverageAvailableMb
    $r = Switch-Profile $ProfileName
    $m.switch_exit = $r.Code; $m.switch_ms = $r.Ms
    $m.ws_frozen_mb = Get-WorkingSetMb $Large.exe
    $series = @()
    for ($i = 1; $i -le $SettleSeconds; $i++) { Start-Sleep -Seconds 1; $series += Get-AvailableMb }
    $m.avail_series_mb = $series
    $m.avail_after_mb = Get-AverageAvailableMb
    $m.avail_gain_mb = [math]::Round($m.avail_after_mb - $m.avail_before_mb, 1)
    $f = Get-FreezeReport @($Large)
    $m.frozen = $f.all
    $beat0 = Get-Beat $Large
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $n = Invoke-Ks switch neutral
    $m.neutral_ms = $sw.ElapsedMilliseconds
    $w = Get-MainWindows $Large | Select-Object -First 1
    if ($w) { $m.window_answer_after_neutral_ms = [KsBeta.Native]::Ping($w.Handle, 10000) }
    if ($beat0 -gt 0) {
        while ((Get-Beat $Large) -eq $beat0 -and $sw.ElapsedMilliseconds -lt 30000) { Start-Sleep -Milliseconds 5 }
        $m.first_heartbeat_after_neutral_ms = $sw.ElapsedMilliseconds
    }
    Start-Sleep -Seconds 2
    $m.ws_after_neutral_mb = Get-WorkingSetMb $Large.exe
    $m
}

function Test-T19 {
    $large = $script:Programs.large
    if (-not $large) { Add-Skip t19 trim 'no "large" program in beta-test.json'; return }
    $null = Start-Tray
    Set-Neutral
    # A fixture that was trimmed before keeps its memory out of RAM: start it afresh.
    if ($large.disposable -and $large.path) { $null = Close-Program $large -Wait }
    $running = Request-Running @($large) 10000
    if (-not $running.Count) { Add-Skip t19 trim "$($large.exe) does not run"; return }
    $gains = @{}
    # Without trim first: after a trim the program's memory stays out of RAM
    # until it touches it again, which would spoil the second measurement.
    foreach ($case in @(@{ id = 'trim-off'; profile = 'beta-trim-off' }, @{ id = 'trim-on'; profile = 'beta-trim-on' })) {
        Invoke-Step t19 $case.id "Available memory with $($case.profile) (T19)" -Freeze -Prepare {
            $m = Measure-Trim $case.profile $large
            $script:Ctx.t19 = $m
            $m
        } -Check {
            param($since)
            $m = $script:Ctx.t19
            $gains[$case.id] = $m.avail_gain_mb
            $note = if ($m.ws_running_mb -lt 50) { "$($large.exe) had only $($m.ws_running_mb) MB in RAM before the switch; the gain says little" }
            @{ ok = ($m.frozen -and $m.switch_exit -eq 0); note = $note }
        } -Ask "Did $($large.exe) react without a noticeable delay after neutral?" -AskNote
        Start-Sleep -Seconds 3
    }
    Invoke-Step t19 compare 'Trim against no trim' -Check {
        param($since)
        $on = $gains['trim-on']; $off = $gains['trim-off']
        @{ ok = ($null -ne $on -and $null -ne $off -and $on -gt $off); m = [ordered]@{ gain_trim_mb = $on; gain_no_trim_mb = $off } }
    }
}

function Test-T18 {
    $el = $script:Programs.elevated
    if (-not $el) { Add-Skip t18 elevated 'no "elevated" program in beta-test.json'; return }
    $null = Start-Tray
    Set-Neutral
    $first = Get-First
    $null = Request-Running @($first)
    Invoke-Step t18 elevated 'An elevated program in a suspend entry (T18)' -Freeze -Human -Prepare {
        if (-not (Get-ProgramProcesses $el.exe).Count -and $el.path -and (Confirm-Yes "   Start $($el.exe) as administrator now (UAC prompt)?")) {
            $params = @{ FilePath = $el.path; Verb = 'RunAs' }
            if ($el.args.Count) { $params.ArgumentList = @($el.args | ForEach-Object { Quote-Arg $_ }) }
            Start-Process @params
        }
        $null
    } -Say @("$($el.exe) must run as administrator now (start it with ""Run as administrator"" if it does not). Then press Enter: the script applies beta-elevated.") -Check {
        param($since)
        $r = Switch-Profile 'beta-elevated'
        $f = Get-FreezeReport @($first)
        $ok = $r.Text -match 'KS102' -and $f.all
        @{ ok = $ok; m = [ordered]@{ exit = $r.Code; output = ($r.Output -join ' | '); other_program = $f.detail } }
    } -Ask 'The tray menu reports that it needs administrator, and the rest of the switch happened?'
    Set-Neutral
}

function Test-T15 {
    $admin = Test-Admin
    Invoke-Step t15 trace 'Programs without an entry are never opened (T15, optional)' -Human -Prepare {
        $null = Start-Tray
        Set-Neutral
        $null = Request-Running $script:Programs.suspend
        [ordered]@{ elevated_session = $admin }
    } -Say @(
        $(if (-not $admin) { 'This needs an administrator session for the trace tool; answer s if you have none.' } else { 'An administrator session: good.' }),
        'Start a trace that records which processes kaltstellen opens (for example Windows Performance Recorder with the Microsoft-Windows-Kernel-Audit-API-Calls provider, OpenProcess events).',
        'Press Enter: the script switches to beta-freeze and back and records the PIDs of the profile programs; stop the trace afterwards.'
    ) -Check {
        param($since)
        $before = @(Get-CimInstance Win32_Process | Where-Object { $_.SessionId -eq $script:SessionId } | ForEach-Object { "$($_.ProcessId) $($_.Name)" })
        $a = Switch-Profile 'beta-freeze'
        $b = Switch-Profile neutral
        $pids = [ordered]@{}
        foreach ($p in $script:Programs.suspend) { $pids[$p.exe] = @(Get-ProgramProcesses $p.exe | ForEach-Object { $_.Id }) }
        $ks = @(Get-OurProcesses | ForEach-Object { $_.Id })
        @{ ok = $null; m = [ordered]@{ window_utc = "$since .. $(Get-UtcStamp)"; kaltstellen_pids = $ks; profile_pids = $pids; processes_in_session = $before.Count; switch_exit = $a.Code; neutral_exit = $b.Code } }
    } -Ask 'The trace shows kaltstellen opening only the PIDs of the profile programs (recorded above)?' -AskNote
}

function Test-Defender {
    Invoke-Step defender history 'Defender (T21)' -Check {
        param($since)
        try {
            $d = @(Get-MpThreatDetection -ErrorAction Stop | Where-Object { ($_.Resources -join ' ') -match 'kaltstellen' })
            @{ ok = ($d.Count -eq 0); m = [ordered]@{ detections_naming_kaltstellen = $d.Count } }
        } catch { @{ ok = $null; m = [ordered]@{ query = "not readable here: $($_.Exception.Message)" } } }
    } -Say @('Windows Security, Virus & threat protection, Protection history: look for anything about kaltstellen.') -Ask 'Nothing about kaltstellen in the protection history?'
}

function Test-Updates {
    $null = Start-Tray
    $settings = Join-Path $script:ConfigDir 'settings.json'
    Invoke-Step updates modes 'Update modes in the menu (manual-checks "Updates" 1)' -Human -Say @(
        'In the tray menu choose "Updates: check and ask", then "Updates: off", then "Updates: install automatically" again.',
        'The check mark moves each time. (Choosing "install automatically" checks GitHub once: that is the product, not this script.)'
    ) -Check {
        param($since)
        $modes = @(Find-Log 'tray: update mode ' $since -All | ForEach-Object { ($_ -split 'update mode ')[-1].Trim() })
        $s = Read-JsonFile $settings
        @{ ok = ($modes -contains 'manual' -and $modes -contains 'off'); m = [ordered]@{ modes_logged = $modes; settings_update = $(if ($s) { $s.update }) } }
    } -Ask 'The check mark moved each time?'

    $uj = Join-Path $script:DataDir 'update.json'
    $had = Test-Path -LiteralPath $uj
    $old = if ($had) { [IO.File]::ReadAllText($uj) } else { $null }
    try {
        Invoke-Step updates offer 'A newer version is offered (step 2)' -Prepare {
            Write-Utf8 $uj '{ "latest": "9.9.9" }'
            $null
        } -Say @(
            'The script wrote update.json naming version 9.9.9 (it does not exist). Open the tray menu:',
            'a release build shows "Install update 9.9.9...", a development build a disabled "Update 9.9.9 available: ..." line. Do NOT click it.'
        ) -Ask 'The menu shows the line as described?'
    } finally {
        if ($had) { Write-Utf8 $uj $old } elseif (Test-Path -LiteralPath $uj) { Remove-Item -LiteralPath $uj -Force }
    }

    if ($script:IsUnattended -or -not (Confirm-Yes '   Is a newer beta released and do you want to test installing it now?')) {
        Add-Skip updates install 'no newer release to install yet (beta-test section 4)'
        return
    }
    $before = (Invoke-Ks --version).Output | Select-Object -First 1
    $oldExe = Join-Path $script:State.install_dir 'kaltstellen.old.exe'
    $oldHash = if (Test-Path -LiteralPath $oldExe) { Get-Sha256 $oldExe } else { $null }
    $null = Request-Running $script:Programs.suspend
    $auto = Confirm-Yes '   Test the automatic install (y) or the install on request with frozen programs (n)?'
    Invoke-Step updates install $(if ($auto) { 'Automatic install while neutral (step 1a)' } else { 'Install on request with frozen programs (step 4)' }) -Freeze -Human -Say $(if ($auto) {
            @('Keep "Updates: install automatically". Apply beta-freeze, then choose "Updates: install automatically" again to check now: nothing is installed while the profile is active.',
              'Then choose Neutral: the update installs without a window, the tray icon disappears briefly and comes back. Wait for it, then press Enter.')
        } else {
            @('Choose "Updates: check and ask". Apply beta-freeze. Then run in another terminal:',
              "  & '$($script:KsExe)' update",
              'Expect: the frozen programs run again, the console says "updated to version ...", one new tray icon appears (not two), its menu offers "Apply last profile". Then press Enter.')
        }) -Check {
        param($since)
        $null = Wait-Until 30 { Get-MainTray }
        $after = (Invoke-Ks --version).Output | Select-Object -First 1
        $trays = @(Get-TrayWindows | Where-Object { (Get-HelperPids) -notcontains [int]$_.Pid }).Count
        $f = Get-FreezeReport $script:Programs.suspend
        $oldNow = if (Test-Path -LiteralPath $oldExe) { Get-Sha256 $oldExe } else { $null }
        Set-State 'version' $after
        @{ ok = ($after -ne $before -and $trays -eq 1 -and $f.none -and $oldNow -and $oldNow -ne $oldHash)
           m = [ordered]@{ version_before = $before; version_after = $after; trays = $trays; frozen = $f.detail; old_exe_next_to_it = [bool]$oldNow } }
    } -Ask 'As described? Did SmartScreen warn when the new tray started (note it)?' -AskNote
}

function Send-SessionEnd($Tray) {
    $q = [KsBeta.Native]::Send($Tray.Handle, $script:WM_QUERYENDSESSION, 0, [int64]$script:ENDSESSION_LOGOFF, 10000)
    $e = [KsBeta.Native]::Send($Tray.Handle, $script:WM_ENDSESSION, 1, [int64]$script:ENDSESSION_LOGOFF, 10000)
    Start-Sleep -Milliseconds 500
    Stop-Process -Id $Tray.Pid -Force -ErrorAction SilentlyContinue
    [ordered]@{ query_end_session_ms = $q; end_session_ms = $e }
}

function Test-Session([string]$Kind) {
    $id = $Kind.ToLowerInvariant()
    if ($script:State.pending) { throw "a session test ($($script:State.pending.test)) waits for 'collect' already" }
    $null = Start-Tray
    Set-Neutral
    $tr = $script:Programs.trigger
    $useTrigger = $false
    if ($id -eq 't7' -and $tr -and $tr.path -and -not $script:IsUnattended) {
        $useTrigger = Confirm-Yes '   Also step 11 of "Automatic triggers": sign out while the trigger has applied its profile?'
    }
    $running = Request-Running ($script:Programs.suspend + $script:Programs.stop)
    $expected = 'beta-session'
    Invoke-Step $id prepare "$(if ($id -eq 't7') { 'Sign-out' } else { 'Shutdown' }) with programs frozen: prepare ($Kind)" -Freeze -Prepare {
        $r = Switch-Profile 'beta-session'
        $m = [ordered]@{ switch_exit = $r.Code }
        if ($useTrigger) {
            Set-TriggersPaused $false
            $s = Get-UtcStamp
            $null = Start-Program $tr
            $null = Wait-Log 'switch to "beta-trigger" finished' $s 25
            $m.trigger_applied = (Get-Active) -eq 'beta-trigger'
        }
        $m
    } -Check {
        param($since)
        $f = Get-FreezeReport $script:Programs.suspend
        @{ ok = $f.all; m = [ordered]@{ frozen = $f.detail; active = (Get-Active) } }
    }
    $boot = ConvertTo-Stamp (Get-CimInstance Win32_OperatingSystem).LastBootUpTime
    $marker = [pscustomobject]@{
        test = $id; since = (Get-UtcStamp (-2000)); expected_offer = $expected; boot = $boot
        simulated = [bool]$SimulateSessionEnd; trigger = $useTrigger; sandbox = [bool]$script:State.sandbox
        frozen = @($script:Programs.suspend | ForEach-Object { $_.exe })
    }
    Set-State 'pending' $marker
    if ($SimulateSessionEnd) {
        Write-Note '   Simulated: the tray gets WM_QUERYENDSESSION and WM_ENDSESSION, then it is ended as Windows would.'
        $sim = Send-SessionEnd (Get-MainTray)
        Add-Result $id simulate 'pass' $true $null $sim 'session end simulated; run collect' (Get-UtcStamp)
        Write-Info '   Now run: beta-test.ps1 collect'
        return
    }
    $word = if ($id -eq 't7') { 'SIGNOUT' } else { 'SHUTDOWN' }
    $what = if ($id -eq 't7') { 'sign you out' } elseif ($Restart) { 'restart Windows' } else { 'shut Windows down' }
    Write-Note "   This will $what NOW. Save everything. After signing in again run: .\scripts\beta-test.ps1 collect"
    if ($id -eq 't20') { Write-Note '   Watch for a "this app is preventing shutdown" screen.' }
    if ($script:IsUnattended) { $typed = '' } else { $typed = (Read-Host "   Type $word to go on (anything else cancels)").Trim() }
    if ($typed -cne $word) {
        Set-State 'pending' $null
        if ($useTrigger) { $null = Request-Closed $tr; Set-TriggersPaused $true }
        Set-Neutral
        Add-Result $id run 'skip' $null $null ([ordered]@{}) 'cancelled before the session ended' (Get-UtcStamp)
        return
    }
    $arg = if ($id -eq 't7') { @('/l') } elseif ($Restart) { @('/r', '/t', '0') } else { @('/s', '/t', '0') }
    & shutdown.exe @arg
}

function Complete-PendingSession {
    $p = $script:State.pending
    if (-not $p) { return }
    $id = $p.test
    Write-Title "== $id / after the session end"
    $lines = Get-LogLines $p.since
    $start = $lines | Where-Object { $_.Contains('tray: session is ending; resuming everything') } | Select-Object -First 1
    $end = $null; $timedOut = $false
    if ($start) {
        $after = $false
        foreach ($l in $lines) {
            if ($l -eq $start) { $after = $true; continue }
            if (-not $after) { continue }
            if ($l.Contains('tray: resuming did not finish in time')) { $end = $l; $timedOut = $true; break }
            if ($l.Contains('tray: end of session finished')) { $end = $l; break }
        }
    }
    $ms = if ($start -and $end) { [int]((Get-LogTime $end) - (Get-LogTime $start)).TotalMilliseconds } else { $null }
    $cleared = $lines | Where-Object { $_ -match 'tray: session ends; \d+ stopped record' } | Select-Object -First 1
    $bootNow = ConvertTo-Stamp (Get-CimInstance Win32_OperatingSystem).LastBootUpTime
    $m = [ordered]@{
        session_ending_line = $(if ($start) { $start.Substring(0, 23) }); resume_end_line = $(if ($end) { $end.Substring(24).Trim() })
        resume_ms = $ms; timed_out = $timedOut; stopped_cleared = $(if ($cleared) { $cleared.Substring(24).Trim() })
        rebooted = ($bootNow -ne $p.boot); simulated = $p.simulated
    }
    Add-Result $id resume $(if ($start -and $end -and -not $timedOut) { 'pass' } else { 'fail' }) ([bool]($start -and $end -and -not $timedOut)) $null $m $(if ($ms) { "resume took $ms ms (the open 4 s wait, open-decisions deferred item 1)" }) $p.since

    $sessionEnd = if ($start) { $start.Substring(0, 24) } else { $p.since }
    $trayRan = [bool](Get-MainTray)
    $startedAfter = Find-Log 'tray: started (pid' $sessionEnd
    if (-not $p.sandbox -and -not $p.simulated) {
        $rv = Get-RunValue $script:RealRunKey
        $ok = $trayRan -and [bool]$startedAfter -and $rv.present
        Add-Result signin at-sign-in $(if ($ok) { 'pass' } else { 'fail' }) $ok $null ([ordered]@{ tray_running = $trayRan; started_line = $(if ($startedAfter) { $startedAfter.Substring(0, 23) }); run_value = $rv.data }) 'the tray started at sign-in' $p.since
    }
    $t0 = Get-UtcStamp (-500)
    if (-not $trayRan) { $null = Start-Tray }
    $offer = Wait-Log 'tray: offering last profile' $sessionEnd 10
    $offered = if ($offer -and $offer -match 'offering last profile "(.+)"') { $Matches[1] } else { $null }
    $active = Get-Active
    $f = Get-FreezeReport ($p.frozen | ForEach-Object { [pscustomobject]@{ exe = $_ } })
    $ok = ($offered -eq $p.expected_offer) -and $active -eq 'neutral' -and -not ($f.detail.Values -match '^frozen')
    $script:Ctx.offer = @{ ok = $ok; m = [ordered]@{ offered = $offered; expected = $p.expected_offer; active = $active; frozen_now = $f.detail } }
    Invoke-Step $id offer 'The last profile is offered, not applied' -Check { param($since) $script:Ctx.offer } -Ask "The tray menu offers ""Apply last profile"" ($($p.expected_offer)) and nothing is frozen?"
    if (-not $p.simulated) {
        $q = if ($id -eq 't7') { 'The sign-out went through without a screen waiting for programs?' } else { 'The shutdown went through without a "this app is preventing shutdown" screen?' }
        Invoke-Step $id not-held 'Not held up' -Ask $q -AskNote
    }
    if ($p.trigger) { Set-TriggersPaused $true }
    Set-State 'pending' $null
}

$script:Catalog = [ordered]@{
    'tray'         = @{ title = 'Tray icon and menu'; ref = 'beta-test 1.1'; fn = 'Test-Tray' }
    'signin'       = @{ title = 'Start at sign-in question'; ref = 'beta-test 1.2'; fn = 'Test-Signin' }
    'window'       = @{ title = 'Main window'; ref = 'beta-test 1.3, manual-checks "Main window"'; fn = 'Test-Window' }
    'failure'      = @{ title = 'Failure markers and errors that resolve'; ref = 'manual-checks 13c, tray 5'; fn = 'Test-Failure' }
    'dragdrop'     = @{ title = 'Drag and drop from Explorer'; ref = 'manual-checks 13d'; fn = 'Test-DragDrop' }
    'descriptions' = @{ title = 'Program descriptions'; ref = 'beta-test 1.4'; fn = 'Test-Descriptions' }
    'hotkeys'      = @{ title = 'Hotkeys (T17)'; ref = 'beta-test 1.5'; fn = 'Test-Hotkeys' }
    'unassigned'   = @{ title = 'Unassigned programs (T22)'; ref = 'beta-test 1.6'; fn = 'Test-Unassigned' }
    'stop'         = @{ title = 'Stop entries and restart on neutral'; ref = 'spec T10/T11'; fn = 'Test-Stop' }
    'triggers'     = @{ title = 'Automatic triggers (incl. 4a, 4b)'; ref = 'beta-test 2'; fn = 'Test-Triggers' }
    't14'          = @{ title = 'Shell stays responsive (T14)'; ref = 'beta-test 3.2'; fn = 'Test-T14' }
    't19'          = @{ title = 'Trim as available memory (T19)'; ref = 'beta-test 3.3'; fn = 'Test-T19' }
    't18'          = @{ title = 'Elevated program (T18)'; ref = 'beta-test 3.4'; fn = 'Test-T18' }
    't13'          = @{ title = 'Long freeze (T13)'; ref = 'beta-test 3.1'; fn = 'Test-T13' }
    't15'          = @{ title = 'Unmatched programs never opened (T15, optional)'; ref = 'beta-test 3.5'; fn = 'Test-T15' }
    'defender'     = @{ title = 'Defender (T21)'; ref = 'beta-test 1.7'; fn = 'Test-Defender' }
    'updates'      = @{ title = 'Updates'; ref = 'beta-test 4'; fn = 'Test-Updates' }
    't7'           = @{ title = 'Sign-out with programs frozen (T7; ends the session)'; ref = 'beta-test 3.6'; fn = 'Test-Session T7'; session = $true }
    't20'          = @{ title = 'Shutdown with programs frozen (T20; ends the session)'; ref = 'beta-test 3.7'; fn = 'Test-Session T20'; session = $true }
}

function Show-TestList {
    $results = Get-Results
    Write-Title 'Tests (run <id>; "all" runs everything except t7 and t20)'
    foreach ($k in $script:Catalog.Keys) {
        $t = $script:Catalog[$k]
        $latest = @{}
        foreach ($r in $results | Where-Object { $_.test -eq $k }) { $latest[$r.step] = $r.outcome }
        $state = if (-not $latest.Count) { 'open' } else {
            $o = @($latest.Values)
            if ($o -contains 'fail') { 'fail' } elseif ($o -contains 'pass') { 'pass' } else { $o[-1] }
        }
        Write-Info ('  {0,-13} {1,-6} {2}  ({3})' -f $k, $state, $t.title, $t.ref)
    }
    if ($script:State.pending) { Write-Note "  pending: $($script:State.pending.test) waits for 'collect'" }
}

function Invoke-Run {
    Use-State
    if ($script:State.phase -ne 'ready') { throw "setup did not finish (phase '$($script:State.phase)'); run teardown, then setup again" }
    $Target = (@($Target) -join ',').Trim(',')
    if (-not $Target -or $Target -eq 'list') { Show-TestList; return }
    $ids = if ($Target -eq 'all') { @($script:Catalog.Keys | Where-Object { -not $script:Catalog[$_].session }) } else { @($Target.Split(',') | ForEach-Object { $_.Trim().ToLowerInvariant() }) }
    foreach ($id in $ids) { if (-not $script:Catalog.Contains($id)) { throw "unknown test '$id'; run 'beta-test.ps1 run list'" } }
    if ($script:State.pending) {
        if (@($ids | Where-Object { $script:Catalog[$_].session }).Count) { throw "a session test ($($script:State.pending.test)) waits for 'collect'; run collect first" }
        Write-Note "A session test ($($script:State.pending.test)) waits for 'collect'; run collect first after signing in again."
    }
    foreach ($id in $ids) {
        $t = $script:Catalog[$id]
        Write-Title "######## ${id}: $($t.title) ($($t.ref))"
        try { Invoke-Expression $t.fn }
        catch {
            if (Test-Abort $_) { Write-Note 'Run stopped. Everything was resumed.'; return }
            Write-Bad "test $id failed: $($_.Exception.Message)"
            Add-Result $id error 'fail' $false $null ([ordered]@{}) ('error: ' + $_.Exception.Message) (Get-UtcStamp)
            try { Invoke-WayOut } catch {}
        }
    }
    Show-TestList
}

# --- collect --------------------------------------------------------------------

function Protect-Text([string]$Text) {
    if (-not $Text) { return $Text }
    $Text = [regex]::Replace($Text, [regex]::Escape($env:USERPROFILE), '%USERPROFILE%', 'IgnoreCase')
    $Text = [regex]::Replace($Text, [regex]::Escape($env:USERPROFILE.Replace('\', '\\')), '%USERPROFILE%', 'IgnoreCase')
    $Text
}

function Invoke-Collect {
    Use-State
    Complete-PendingSession
    $results = Get-Results
    $stage = Join-Path $script:BackupRoot 'staging'
    Remove-Tree $stage
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    try {
        $put = { param($name, $text) Write-Utf8 (Join-Path $stage $name) (Protect-Text $text) }
        & $put 'results.json' ((ConvertTo-Json -InputObject @($results) -Depth 8))
        $logFile = Join-Path $script:Session 'results.log'
        & $put 'results.log' $(if (Test-Path -LiteralPath $logFile) { [IO.File]::ReadAllText($logFile) } else { '' })
        # The log lines inside the windows of the recorded steps.
        $fmt = "yyyy-MM-dd'T'HH:mm:ss.fff'Z'"
        $windows = @($results | Where-Object { $_.since -and $_.until } | ForEach-Object {
            [pscustomobject]@{ from = $_.since; to = (ConvertFrom-UtcStamp $_.until).AddSeconds(2).ToString($fmt, $script:Invariant) }
        } | Sort-Object from)
        $merged = New-Object System.Collections.Generic.List[object]
        foreach ($w in $windows) {
            if ($merged.Count -and [string]::CompareOrdinal($w.from, $merged[$merged.Count - 1].to) -le 0) {
                if ([string]::CompareOrdinal($w.to, $merged[$merged.Count - 1].to) -gt 0) { $merged[$merged.Count - 1].to = $w.to }
            } else { $merged.Add([pscustomobject]@{ from = $w.from; to = $w.to }) }
        }
        $keep = New-Object System.Collections.Generic.List[string]
        $i = 0
        foreach ($l in Get-LogLines '0000') {
            $ts = $l.Substring(0, [math]::Min(24, $l.Length))
            while ($i -lt $merged.Count -and [string]::CompareOrdinal($ts, $merged[$i].to) -gt 0) { $i++ }
            if ($i -ge $merged.Count) { break }
            if ([string]::CompareOrdinal($ts, $merged[$i].from) -ge 0) { $keep.Add($l) }
        }
        & $put 'kaltstellen-log-excerpt.txt' (($keep.ToArray()) -join "`r`n")
        $os = Get-CimInstance Win32_OperatingSystem
        $ubr = try { (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').UBR } catch { $null }
        $wv = try { (Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction Stop).pv } catch { $null }
        $ver = (Invoke-Ks --version).Output | Select-Object -First 1
        $info = [ordered]@{
            kaltstellen = $ver; installed_sha256 = $script:State.install.sha256; source = $script:State.install.source
            windows = $os.Caption; version = $os.Version; build = "$($os.BuildNumber).$ubr"; os_architecture = $os.OSArchitecture
            processor_architecture = $env:PROCESSOR_ARCHITECTURE; webview2_runtime = $wv
            powershell = $PSVersionTable.PSVersion.ToString(); sandbox = $script:State.sandbox; collected_utc = (Get-UtcStamp)
        }
        & $put 'system.json' ($info | ConvertTo-Json)
        foreach ($f in @((Join-Path $script:DataDir 'state.json'), (Join-Path $script:ConfigDir 'settings.json'))) {
            if (Test-Path -LiteralPath $f) { & $put (Split-Path $f -Leaf) ([IO.File]::ReadAllText($f)) }
        }
        $dir = if ($ReportDir) { [IO.Path]::GetFullPath($ReportDir) } else {
            $d = [Environment]::GetFolderPath('Desktop')
            if ($d -and (Test-Path -LiteralPath $d)) { $d } else { $script:Session }
        }
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $zip = Join-Path $dir ("kaltstellen-beta-report-{0}.zip" -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
        [IO.Compression.ZipFile]::CreateFromDirectory($stage, $zip)
        $pass = @($results | Where-Object { $_.outcome -eq 'pass' }).Count
        $fail = @($results | Where-Object { $_.outcome -eq 'fail' }).Count
        Write-Title 'Report'
        Write-Info "  $($results.Count) step(s): $pass pass, $fail fail, $($results.Count - $pass - $fail) skipped or n/a"
        Write-Good "  $zip"
        Write-Info '  It holds the results, the kaltstellen.log lines of the test steps, version, Windows version, state.json and settings.json;'
        Write-Info '  your profile folder is written as %USERPROFILE%. Look at it before you send it.'
    } finally { Remove-Tree $stage }
}

# --- teardown -------------------------------------------------------------------

function Restore-Folder($Backup) {
    $target = $Backup.path
    if ($Backup.existed) {
        if (Test-Path -LiteralPath $target) { Clear-Folder $target } else { New-Item -ItemType Directory -Path $target | Out-Null }
        if (Test-Path -LiteralPath $Backup.backup) {
            Get-ChildItem -LiteralPath $Backup.backup -Force | Copy-Item -Destination $target -Recurse -Force
        }
        $problem = Test-Manifest $target $Backup.files
        if ($problem) { throw "restored $target does not match its backup: $problem" }
        "restored $target ($(@(Get-List $Backup.files).Count) file(s), verified)"
    } elseif (Test-Path -LiteralPath $target) {
        Remove-Tree $target
        "removed $target (it did not exist before the test)"
    } else { "$target did not exist and does not exist" }
}

function Restore-RunValue($Saved) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($script:RealRunKey)
    try {
        if ($Saved.present) {
            $kind = [Microsoft.Win32.RegistryValueKind]$Saved.kind
            $key.SetValue($script:RunValue, $Saved.data, $kind)
            "Run value kaltstellen restored: $($Saved.data)"
        } elseif ($null -ne $key.GetValue($script:RunValue)) {
            $key.DeleteValue($script:RunValue)
            'Run value kaltstellen removed (it did not exist before the test)'
        } else { 'Run value kaltstellen: absent, as before' }
    } finally { $key.Close() }
}

function Invoke-Teardown {
    $script:State = Read-State
    if (-not $script:State) { Write-Info "Nothing to tear down: no beta test is set up in $($script:BackupRoot)."; return }
    $s = $script:State
    $script:KsExe = $s.exe; $script:ConfigDir = $s.config_dir; $script:DataDir = $s.data_dir
    $script:TestDir = $s.test_dir; $script:Session = $s.session_dir
    Set-KsEnvironment
    $haveExe = $script:KsExe -and (Test-Path -LiteralPath $script:KsExe)
    Write-Title 'Teardown plan'
    Write-Info "  quit kaltstellen started from $($script:KsExe) after 'recover' and 'neutral'"
    Write-Info "  end the programs the test started ($(@(Get-List $s.started).Count)); disposable ones and those in $($s.temp_root) are killed, others asked to close"
    if (-not $s.sandbox) {
        if ($s.backup) {
            Write-Info "  restore $($s.backup.config.path) and $($s.backup.data.path) from $($s.session_dir)\backup"
            Write-Info "  restore the Run value ($(if ($s.backup.run_value.present) { $s.backup.run_value.data } else { 'absent' }))"
        } else { Write-Info '  no backup was taken (setup stopped before), nothing to restore' }
    }
    Write-Info "  remove $($s.temp_root) and HKCU\$($s.registry_root)"
    if ($Uninstall) { Write-Info "  uninstall $($script:KsExe)$(if (@(Get-List $s.install.preexisting).Count) { ' and put the earlier copy back' })" }
    if ($DryRun) { Write-Info ''; Write-Info '[dry run] nothing was changed.'; return }

    $done = New-Object System.Collections.Generic.List[string]
    $problems = New-Object System.Collections.Generic.List[string]
    # 1. kaltstellen: resume, neutral, quit.
    if ($haveExe) {
        foreach ($w in @(Get-OurProcesses | ForEach-Object { $_.Id } | ForEach-Object { Get-WindowsOf @($_) 'kaltstellen-main' })) {
            $null = [KsBeta.Native]::PostMessage($w.Handle, $script:WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
        }
        $r = Invoke-Ks recover
        $n = Invoke-Ks switch neutral
        $done.Add("recover: exit $($r.Code); neutral: exit $($n.Code)")
        foreach ($t in Get-TrayWindows) {
            if (Stop-TrayWindow $t 25) { $done.Add("tray $($t.Pid) quit") } else { $done.Add("tray $($t.Pid) ended") }
        }
        $r2 = Invoke-Ks recover
        if ($r2.Code -ne 0) { $problems.Add("recover after quit: exit $($r2.Code): $($r2.Text)") }
    } elseif ($s.phase -eq 'ready') {
        $done.Add("WARNING: $($script:KsExe) is missing, 'recover' could not run; if anything is still frozen, run recover with another kaltstellen.exe")
    }
    foreach ($p in Get-OurProcesses) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue; $done.Add("ended kaltstellen $($p.Id)") }
    Start-Sleep -Milliseconds 800
    # 2. Programs the test started.
    foreach ($e in Get-List $s.started) {
        $p = Get-Process -Id $e.pid -ErrorAction SilentlyContinue
        if (-not $p) { continue }
        $ticks = try { $p.StartTime.ToUniversalTime().Ticks } catch { 0 }
        if ($e.start_ticks -and $ticks -ne $e.start_ticks) { continue }
        $inTemp = $p.Path -and $s.temp_root -and $p.Path.StartsWith($s.temp_root, [StringComparison]::OrdinalIgnoreCase)
        if ($e.disposable -or $inTemp) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue; $done.Add("ended $($e.exe) ($($e.pid))") }
        else {
            $null = $p.CloseMainWindow()
            if (-not $p.WaitForExit(5000)) { $problems.Add("$($e.exe) ($($e.pid)) was started by the test and still runs; close it yourself") }
            else { $done.Add("closed $($e.exe) ($($e.pid))") }
        }
    }
    # Disposable programs (test fixtures) also when kaltstellen started them (restart_on_neutral).
    if ($s.programs_file -and (Test-Path -LiteralPath $s.programs_file)) {
        $pl = Read-ProgramList $s.programs_file -NoFail
        foreach ($prog in @($pl.suspend) + @($pl.stop) + @($pl.trigger, $pl.large, $pl.tray_program)) {
            if (-not $prog -or -not $prog.disposable) { continue }
            foreach ($p in Get-ProgramProcesses $prog.exe) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue; $done.Add("ended $($prog.exe) ($($p.Id))") }
        }
    }
    if ($s.temp_root) {
        foreach ($p in Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($s.temp_root + '\', [StringComparison]::OrdinalIgnoreCase) }) {
            Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue; $done.Add("ended $($p.Name) ($($p.Id)) from the test folder")
        }
    }
    Start-Sleep -Milliseconds 800
    # 3. The real configuration, data and Run value.
    if (-not $s.sandbox -and $s.backup) {
        $keep = Join-Path $s.session_dir 'at-teardown'
        New-Item -ItemType Directory -Force -Path $keep | Out-Null
        foreach ($f in @('config.toml', 'settings.json') | ForEach-Object { Join-Path $s.config_dir $_ }) { if (Test-Path -LiteralPath $f) { Copy-Item -LiteralPath $f -Destination $keep -Force } }
        foreach ($f in @('kaltstellen.log', 'kaltstellen.log.1', 'kaltstellen.log.2', 'state.json', 'seen.json', 'update.json') | ForEach-Object { Join-Path $s.data_dir $_ }) { if (Test-Path -LiteralPath $f) { Copy-Item -LiteralPath $f -Destination $keep -Force } }
        foreach ($b in @($s.backup.config, $s.backup.data)) {
            try { $done.Add((Restore-Folder $b)) } catch { $problems.Add($_.Exception.Message) }
        }
        try { $done.Add((Restore-RunValue $s.backup.run_value)) } catch { $problems.Add("Run value: $($_.Exception.Message)") }
    } elseif (-not $s.sandbox -and -not $s.backup) {
        $done.Add('no backup had been taken: the real configuration was not changed')
    }
    # 4. Test registry keys and folders.
    if ($s.registry_root -and $s.registry_root -like 'Software\kaltstellen-beta-*') {
        if (Test-Path "HKCU:\$($s.registry_root)") { Remove-Item -LiteralPath "HKCU:\$($s.registry_root)" -Recurse -Force; $done.Add("removed HKCU\$($s.registry_root)") }
    }
    $install = $s.install
    $inTempInstall = $s.temp_root -and $s.install_dir.StartsWith($s.temp_root, [StringComparison]::OrdinalIgnoreCase)
    if ($Uninstall -and -not $inTempInstall -and $install) {
        foreach ($n in 'kaltstellen.exe', 'kaltstellen.old.exe') {
            $f = Join-Path $s.install_dir $n
            $pre = @(Get-List $install.preexisting | Where-Object { $_.name -eq $n })
            $saved = Join-Path $s.session_dir "backup\install\$n"
            try {
                if ($pre.Count -and (Test-Path -LiteralPath $saved)) { Copy-Item -LiteralPath $saved -Destination $f -Force; $done.Add("put back the earlier $f") }
                elseif (Test-Path -LiteralPath $f) { Remove-Item -LiteralPath $f -Force; $done.Add("removed $f") }
            } catch { $problems.Add("$f`: $($_.Exception.Message)") }
        }
        if ($install.created_dir -and (Test-Path -LiteralPath $s.install_dir) -and -not (Get-ChildItem -LiteralPath $s.install_dir -Force)) {
            Remove-Item -LiteralPath $s.install_dir -Force; $done.Add("removed $($s.install_dir)")
        }
    }
    if ($s.temp_root -and (Test-Path -LiteralPath $s.temp_root)) {
        try { Remove-Tree $s.temp_root; $done.Add("removed $($s.temp_root)") } catch { $problems.Add("$($s.temp_root): $($_.Exception.Message)") }
    }
    Write-Title 'Teardown'
    foreach ($d in $done) { Write-Good "  $d" }
    if ($problems.Count) {
        foreach ($p in $problems) { Write-Bad "  $p" }
        Write-Bad '  Teardown is not complete; fix the above and run teardown again (it is safe to repeat).'
        $script:ExitCode = 1
        return
    }
    if (-not $Uninstall -and -not $inTempInstall) { Write-Info "  kept $($script:KsExe) (teardown -Uninstall removes it)" }
    Move-Item -LiteralPath $script:StateFile -Destination (Join-Path $s.session_dir 'state.done.json') -Force
    Write-Info "  results and backups stay in $($s.session_dir)"
}

function Show-Status {
    $s = Read-State
    if (-not $s) { Write-Info "No beta test is set up in $($script:BackupRoot)."; return }
    Use-State
    Write-Info "phase $($s.phase), $(if ($s.sandbox) { 'sandbox' } else { 'real configuration' }), set up $($s.setup_utc)"
    Write-Info "executable $($s.exe) ($($s.version))"
    Write-Info "configuration $($s.config_dir); data $($s.data_dir)"
    Write-Info "results $($s.session_dir)"
    Show-TestList
}

# --- main -------------------------------------------------------------------------

$script:ExitCode = 0
try {
    Read-AnswersFile
    switch ($Command) {
        'setup' { Invoke-Setup }
        'run' { Invoke-Run }
        'collect' { Invoke-Collect }
        'teardown' { Invoke-Teardown }
        'status' { Show-Status }
        default { Get-Help $PSCommandPath -Detailed }
    }
} catch {
    if (-not (Test-Abort $_)) {
        Write-Bad "error: $($_.Exception.Message)"
        $script:ExitCode = 1
    }
} finally {
    Restore-Environment
}
exit $script:ExitCode
