<#
.SYNOPSIS
Guided beta test of kaltstellen on the owner's machine (docs/beta-test.md).

.DESCRIPTION
Installs a kaltstellen release (or a local build), backs up the real
configuration, data folder and the start-at-sign-in value, writes a test
configuration built from test programs the script brings itself (or the
programs listed in an optional beta-test.json), and walks
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
down unless you type the word it asks for. The script's only network
access is the release download in `setup`; the test tray itself may check
GitHub for updates unless the update mode is off.

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

.PARAMETER KeepBeta
With teardown: when setup replaced your own kaltstellen.exe, keep the beta
there and finish (without it teardown waits for -Uninstall or -KeepBeta).

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
setup: kept for older command lines; setup no longer opens Notepad.

.PARAMETER FreezeMinutes
t13: how long the programs stay frozen (default 10).

.PARAMETER SettleSeconds
t19: seconds of memory samples after each switch (default 20).

.PARAMETER ProgramList
Program list to use instead of <backup root>\beta-test.json. Without either
file the test programs the script brings are used.

.PARAMETER Root
Backup root instead of %LOCALAPPDATA%\kaltstellen-beta-backup (tests).

.PARAMETER NoProgramLists
collect: leave the output of "kaltstellen list" and "status --unassigned"
(names of programs that ran) out of the report.

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
    [switch]$KeepBeta,
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
    [switch]$NoProgramLists,
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
$script:SessionEnding = $false

# --- Windows API ------------------------------------------------------------

# The namespace carries the script's version: a console that ran an older
# copy of this script keeps its types, which must not be reused.
if (-not ('KsBetaV3.Native' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
namespace KsBetaV3 {
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
            var ok = SendMessageTimeout(h, msg, Word(w), Word(l), 2, timeoutMs, out r);
            return ok == IntPtr.Zero ? -1 : sw.ElapsedMilliseconds;
        }
        // The low 32 bits in a 32-bit process (0x80000000 does not fit an int there).
        static IntPtr Word(long v) { return IntPtr.Size == 8 ? new IntPtr(v) : new IntPtr(unchecked((int)v)); }
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern uint GetLongPathName(string s, StringBuilder l, uint n);
        // C:\Users\ABCDEF~1\... -> the long form; the input if Windows cannot say.
        public static string LongPath(string path) {
            var b = new StringBuilder(1024);
            var n = GetLongPathName(path, b, (uint)b.Capacity);
            return (n == 0 || n >= b.Capacity) ? path : b.ToString();
        }
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern uint GetShortPathName(string l, StringBuilder s, uint n);
        public static string ShortPath(string path) {
            var b = new StringBuilder(1024);
            var n = GetShortPathName(path, b, (uint)b.Capacity);
            return (n == 0 || n >= b.Capacity) ? path : b.ToString();
        }
        [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool IsWow64Process2(IntPtr p, out ushort process, out ushort native);
        // IMAGE_FILE_MACHINE_* of the machine itself, 0 where IsWow64Process2 is missing.
        public static int NativeMachine() {
            try { ushort p, n; return IsWow64Process2(GetCurrentProcess(), out p, out n) ? n : 0; }
            catch (EntryPointNotFoundException) { return 0; }
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
        // INPUT is a union whose largest member (MOUSEINPUT) is 8 bytes longer than KEYBDINPUT;
        // two uints keep the size right also in 32-bit PowerShell (28 bytes there, 40 in 64-bit).
        [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public KEYBDINPUT ki; public uint pad1, pad2; }
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
    $long = Get-LongPath $full
    foreach ($root in $allowed) {
        $r = Get-LongPath $root
        if ((Test-Below $full $r) -or (Test-Below $long $r)) { return }
    }
    throw "refusing to remove ${full}: not a path this script created or backed up"
}

function Get-LongPath([string]$Path) {
    if (-not $Path) { return $Path }
    $full = [IO.Path]::GetFullPath($Path)
    # Long form of the part that exists (a path to be created keeps its tail).
    $head = $full; $tail = ''
    while ($head -and -not (Test-Path -LiteralPath $head)) {
        $tail = '\' + [IO.Path]::GetFileName($head) + $tail
        $head = [IO.Path]::GetDirectoryName($head)
    }
    if (-not $head) { return $full }
    ([KsBetaV3.Native]::LongPath($head)).TrimEnd('\') + $tail
}

# True if $Path is $Root or lies below it.
function Test-Below([string]$Path, [string]$Root) {
    if (-not $Path -or -not $Root) { return $false }
    $r = $Root.TrimEnd('\')
    $Path -ieq $r -or $Path.StartsWith($r + '\', [StringComparison]::OrdinalIgnoreCase)
}

# Deletes a tree without following junctions or symbolic links: a link is
# removed itself, never what it points to.
function Remove-TreeItem([IO.FileSystemInfo]$Item) {
    $isLink = ($Item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0
    if (($Item.Attributes -band [IO.FileAttributes]::ReadOnly) -ne 0) {
        $Item.Attributes = $Item.Attributes -band (-bnot [IO.FileAttributes]::ReadOnly)
    }
    if ($Item -is [IO.DirectoryInfo]) {
        if (-not $isLink) {
            foreach ($child in $Item.GetFileSystemInfos()) { Remove-TreeItem $child }
        }
        [IO.Directory]::Delete($Item.FullName, $false)
    } else {
        [IO.File]::Delete($Item.FullName)
    }
}

function Remove-Tree([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    Assert-Removable $Path
    for ($i = 0; $i -lt 5; $i++) {
        try {
            $item = Get-Item -LiteralPath $Path -Force
            Remove-TreeItem $item
            return
        } catch { if ($i -eq 4) { throw }; Start-Sleep -Milliseconds 800 }
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
        $script:Programs = Read-ProgramList $script:State.programs_file -TestDir $script:State.test_dir -NoFail
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
    Write-Note '   This step freezes or closes the test programs (or the programs of your beta-test.json): save your work in them first.'
    Write-Note "   Way out: answer r at any prompt, or in another terminal: $(Get-WayOutCommand)"
}

# --- Results ------------------------------------------------------------------

function Add-Result {
    param([string]$TestId, [string]$StepId, [string]$Outcome, $Auto, $Eyes, $Measure, [string]$Note, [string]$Since, [string]$Until)
    if (-not $Until) { $Until = Get-UtcStamp }
    $rec = [ordered]@{
        test = $TestId; step = $StepId; outcome = $Outcome
        auto = $Auto; eyes = $Eyes
        measure = $Measure; note = $Note
        since = $Since; until = $Until; time = (Get-UtcStamp)
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
    @([KsBetaV3.Native]::Windows() | Where-Object { ($Pids -contains [int]$_.Pid) -and (-not $Class -or $_.Class -eq $Class) })
}

function Get-TrayWindows {
    $ours = @(Get-OurProcesses | ForEach-Object { $_.Id })
    if (-not $ours.Count) { return @() }
    @(Get-WindowsOf $ours 'kaltstellen-tray')
}

# Helper trays: { pid, start_ticks } of trays the script started itself.
function Get-HelperPids { @(Get-List $script:State.helper_pids | ForEach-Object { if ($_.pid) { [int]$_.pid } else { [int]$_ } }) }

function Get-StartTicks($Process) { try { $Process.StartTime.ToUniversalTime().Ticks } catch { 0 } }

# The process with this PID if it is still the one recorded: same start time
# and, when given, the same executable.
function Get-RecordedProcess([int]$Id, [int64]$StartTicks, [string]$Path) {
    $p = Get-Process -Id $Id -ErrorAction SilentlyContinue
    if (-not $p) { return $null }
    if ($StartTicks -and (Get-StartTicks $p) -ne $StartTicks) { return $null }
    if ($Path -and -not ($p.Path -and (Get-LongPath $p.Path) -ieq (Get-LongPath $Path))) { return $null }
    $p
}

# Fallback for Get-MainTray: the PID in the last "tray: started (pid N)" line
# of the test's log, searched from the end of the newest file; read again
# only when that file changed.
$script:TrayPidCache = @{ key = $null; pid = $null }
$script:TrayStartRegex = New-Object regex 'tray: started \(pid (\d+)\)', 'RightToLeft'
function Get-LoggedTrayPid {
    $f = Join-Path $script:DataDir 'kaltstellen.log'
    if (-not (Test-Path -LiteralPath $f)) { return $null }
    $i = Get-Item -LiteralPath $f
    $key = "$($i.Length) $($i.LastWriteTimeUtc.Ticks)"
    if ($script:TrayPidCache.key -ne $key) {
        $found = $null
        foreach ($name in 'kaltstellen.log', 'kaltstellen.log.1') {
            try {
                $fs = [IO.File]::Open((Join-Path $script:DataDir $name), 'Open', 'Read', 'ReadWrite, Delete')
                try { $text = (New-Object IO.StreamReader($fs, $script:Utf8)).ReadToEnd() } finally { $fs.Dispose() }
                $m = $script:TrayStartRegex.Match($text)
                if ($m.Success) { $found = [int]$m.Groups[1].Value; break }
            } catch { }
        }
        $script:TrayPidCache = @{ key = $key; pid = $found }
    }
    $script:TrayPidCache.pid
}
function Get-MainTray {
    $helpers = Get-HelperPids
    $trays = @(Get-TrayWindows | Where-Object { $helpers -notcontains [int]$_.Pid })
    if ($script:State -and $script:State.sandbox) {
        # In a sandbox: the tray the script started (PID and start time), or
        # else (one the updater started, say) the one that wrote the last
        # start line to the sandbox log.
        $mine = $script:State.main_tray
        $recorded = if ($mine) { Get-RecordedProcess $mine.pid $mine.start_ticks $script:KsExe } else { $null }
        $want = if ($recorded) { $recorded.Id } else { Get-LoggedTrayPid }
        $trays = @($trays | Where-Object { [int]$_.Pid -eq $want })
    }
    $trays | Select-Object -First 1
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
    Set-State 'main_tray' ([pscustomobject]@{ pid = $p.Id; start_ticks = (Get-StartTicks $p) })
    $t = Wait-Until 20 { Get-MainTray }
    $null = Wait-Log 'tray: pipe ready' $since 10
    if (-not $t) { throw "the tray did not start (pid $($p.Id)); see $($script:DataDir)\kaltstellen.log" }
    $t
}

function Stop-TrayWindow($Tray, [int]$TimeoutS = 25) {
    if (-not $Tray) { return $true }
    $null = [KsBetaV3.Native]::PostMessage($Tray.Handle, $script:WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
    $gone = Wait-Until $TimeoutS { -not (Get-Process -Id $Tray.Pid -ErrorAction SilentlyContinue) }
    if (-not $gone) {
        Write-Note "   the tray (pid $($Tray.Pid)) did not quit in $TimeoutS s; ending it"
        Stop-Process -Id $Tray.Pid -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 500
        # A killed tray resumes nothing: recover does.
        $r = Invoke-Ks recover
        Write-Note "   recover after ending it: exit $($r.Code)"
        return $false
    }
    $true
}

# What the installed build can do: "kaltstellen help" lists its commands,
# "<command> --help" its options. Asked once per run.
$script:Supports = @{}
function Test-KsSupports([string]$Command, [string]$Option) {
    $key = "$Command $Option"
    if (-not $script:Supports.ContainsKey($key)) {
        $r = if ($Option) { Invoke-Ks $Command --help } else { Invoke-Ks help }
        $pattern = if ($Option) { '(^|\s)' + [regex]::Escape($Option) + '\b' } else { '(?m)^\s+' + [regex]::Escape($Command) + '\s' }
        $script:Supports[$key] = ($r.Code -eq 0 -and $r.Text -match $pattern)
    }
    $script:Supports[$key]
}

# Quits the main tray as its menu's Quit does (L2: it resumes what it froze):
# "kaltstellen quit" where the build has it, else WM_CLOSE to its window.
function Stop-Tray {
    $t = Get-MainTray
    if (-not $t) { return }
    if (Test-KsSupports quit) {
        $q = Invoke-Ks quit
        if (Wait-Until 25 { -not (Get-Process -Id $t.Pid -ErrorAction SilentlyContinue) }) { return }
        Write-Note "   kaltstellen quit: exit $($q.Code); closing the tray window instead"
    }
    $null = Stop-TrayWindow $t
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

# Automatic switching on or off (setup starts it paused, so a trigger program
# you happen to start freezes nothing before test 'triggers'). With
# "kaltstellen automation" the running tray takes it over at once and writes
# settings.json itself; without a tray the command writes the file. A build
# without that command: the file, read by the tray when it starts again.
function Set-TriggersPaused([bool]$Paused) {
    if (Test-KsSupports automation) {
        $r = Invoke-Ks automation $(if ($Paused) { 'off' } else { 'on' })
        if ($r.Code -ne 0) { throw "kaltstellen automation: exit $($r.Code): $($r.Text)" }
        return
    }
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
    # The writer replaces the file every 100 ms: a read that collides is tried again.
    for ($i = 0; $i -lt 5; $i++) {
        try { return [int64]([IO.File]::ReadAllText((Get-BeatFile $Program)).Trim()) } catch { Start-Sleep -Milliseconds 20 }
    }
    0
}

function Register-Started($Process, $Program) {
    $entry = [pscustomobject]@{
        pid = $Process.Id; exe = $Program.exe; path = $Program.path
        start_ticks = (Get-StartTicks $Process)
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

# Processes of this program that the script started itself: PID and start
# time as recorded in the state file.
function Get-StartedProcesses($Program) {
    @(foreach ($e in Get-List $script:State.started) {
        if ($e.exe -ine $Program.exe) { continue }
        $p = Get-RecordedProcess $e.pid $e.start_ticks $null
        if ($p) { $p }
    })
}

# Ends what the script started of a program: killed when disposable (test
# fixtures), otherwise asked to close (WM_CLOSE to its windows). Other
# processes of the same name are never touched here.
function Close-Program($Program, [switch]$Wait) {
    $procs = Get-StartedProcesses $Program
    if (-not $procs.Count) { return $true }
    Write-Info "   $(if ($Program.disposable) { 'ending' } else { 'closing' }) $($Program.exe) (pid $(($procs | ForEach-Object { $_.Id }) -join ', '))"
    if ($Program.disposable) { $procs | Stop-Process -Force -ErrorAction SilentlyContinue }
    else { foreach ($p in $procs) { $null = $p.CloseMainWindow() } }
    if (-not $Wait) { return $true }
    $ids = @($procs | ForEach-Object { $_.Id })
    $gone = [bool](Wait-Until 15 { -not @($ids | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }).Count })
    if (-not $gone) { Write-Note "   $($Program.exe) did not end: pid $((@($ids | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })) -join ', ')" }
    $gone
}

# Closes what the script started; for anything else of that name the owner
# decides (never forced).
function Request-Closed($Program) {
    $null = Close-Program $Program -Wait
    if (-not (Get-ProgramProcesses $Program.exe).Count) { return $true }
    if ($script:IsUnattended) { Write-Note "   $($Program.exe) runs and was not started by this script: left alone"; return $false }
    while ((Get-ProgramProcesses $Program.exe).Count) {
        $a = (Read-Host "   $($Program.exe) runs (not started by this script). Close it yourself, then press Enter (s = go on anyway)").Trim().ToLowerInvariant()
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

# The trigger "builtin": a renamed copy of ping.exe in the test folder that
# runs until it is ended, so the trigger test never touches a program of yours.
$script:BuiltinTrigger = 'ks-beta-trigger.exe'

# The test programs the script brings: one executable, compiled from
# beta-test-programs.cs at setup and copied to these names (what each does is
# chosen by --role, see the source). "behavior" is what the step "stop" expects
# of a program of the stop list: it closes ('closes'), refuses and stays
# ('stays', on_timeout keep) or is ended by kaltstellen ('killed').
$script:BuiltinSource = 'beta-test-programs.cs'
$script:BuiltinExe = 'ks-beta-prog.exe'
$script:BuiltinPrograms = @{
    suspend = @(
        @{ exe = 'ks-beta-freeze1.exe'; role = 'beat' }
        @{ exe = 'ks-beta-freeze2.exe'; role = 'beat' }
    )
    stop = @(
        @{ exe = 'ks-beta-stop.exe'; role = 'beat'; behavior = 'closes'; on_timeout = 'keep' }
        @{ exe = 'ks-beta-stubborn.exe'; role = 'stubborn'; behavior = 'stays'; on_timeout = 'keep' }
        @{ exe = 'ks-beta-stubborn-kill.exe'; role = 'stubborn'; behavior = 'killed'; on_timeout = 'kill' }
    )
    large = @{ exe = 'ks-beta-large.exe'; role = 'beat'; args = @('--alloc-mb', '300') }
    tray_program = @{ exe = 'ks-beta-tray.exe'; role = 'tray' }
}

# The list used when there is no beta-test.json: everything built in.
$script:DefaultProgramList = '{ "suspend": "builtin", "stop": "builtin", "trigger": "builtin", "large": "builtin", "tray_program": "builtin" }'

# A program of the built-in set as the lists use it. Its file exists after
# setup compiled it; the heartbeat file is named by Start-Program.
function Get-BuiltinProgram($Spec, [string]$TestDir, [string]$ListRole) {
    $roleArgs = @('--role', $Spec.role, '--title', [IO.Path]::GetFileNameWithoutExtension($Spec.exe), '--beat', '{heartbeat}')
    [pscustomobject]@{
        exe = $Spec.exe; path = (Join-Path $TestDir $Spec.exe)
        args = @($roleArgs + @($Spec.args | Where-Object { $null -ne $_ }))
        disposable = $true; hidden = $false
        restart = $true
        on_timeout = $(if ($Spec.on_timeout) { $Spec.on_timeout } else { 'keep' })
        behavior = $Spec.behavior
        role = $ListRole; compiled = $true
    }
}

function Read-ProgramList([string]$Path, [string]$TestDir, [switch]$NoFail) {
    $errors = New-Object System.Collections.Generic.List[string]
    if ($Path) {
        try { $j = ConvertFrom-JsonText (Get-Content -LiteralPath $Path -Raw -Encoding UTF8) }
        catch { throw "$Path is not valid JSON: $($_.Exception.Message)" }
    } else {
        $Path = 'the built-in program list'
        $j = ConvertFrom-JsonText $script:DefaultProgramList
    }
    $seen = @{}
    # "builtin" in place of a program (large, tray_program) or a list (suspend, stop).
    $builtinOf = {
        param($value, [string]$key, [string]$role)
        if ($value -isnot [string]) { return $null }
        if ($value -ne 'builtin') { $errors.Add("$key`: 'builtin' or program entries, not '$value'"); return , @() }
        $specs = @($script:BuiltinPrograms[$key])
        , @(foreach ($sp in $specs) { Get-BuiltinProgram $sp $TestDir $role })
    }
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
    $b = & $builtinOf $j.suspend 'suspend' 'suspend'
    $suspend = if ($null -ne $b) { @($b) } else { @(Get-List $j.suspend | ForEach-Object { & $norm $_ 'suspend' } | Where-Object { $_ }) }
    $b = & $builtinOf $j.stop 'stop' 'stop'
    $stop = if ($null -ne $b) { @($b) } else { @(Get-List $j.stop | ForEach-Object { & $norm $_ 'stop' } | Where-Object { $_ }) }
    if (-not $suspend.Count) { $errors.Add('suspend: list at least one program that may be frozen') }
    foreach ($p in $suspend + $stop) {
        $k = $p.exe.ToLowerInvariant()
        if ($seen.ContainsKey($k)) { $errors.Add("$($p.exe) is listed twice (suspend and stop exclude each other)") }
        $seen[$k] = $p.role
    }
    if ($j.trigger -is [string] -and $j.trigger -eq 'builtin') {
        $trigger = [pscustomobject]@{
            exe = $script:BuiltinTrigger; path = (Join-Path $TestDir $script:BuiltinTrigger); args = @('-t', '127.0.0.1')
            disposable = $true; hidden = $true; restart = $true; on_timeout = 'keep'; role = 'trigger'; builtin = $true
        }
    } elseif ($j.trigger -is [string]) {
        $errors.Add("trigger: 'builtin' or an object with exe and path, not '$($j.trigger)'"); $trigger = $null
    } else { $trigger = & $norm $j.trigger 'trigger' }
    if ($trigger -and $seen.ContainsKey($trigger.exe.ToLowerInvariant())) { $errors.Add("trigger $($trigger.exe) must not be frozen or stopped (it is in $($seen[$trigger.exe.ToLowerInvariant()]))") }
    $b = & $builtinOf $j.large 'large' 'large'
    $large = if ($null -ne $b) { $b[0] } else { & $norm $j.large 'large' }
    if ($large -and $seen[$large.exe.ToLowerInvariant()] -eq 'stop') { $errors.Add("large $($large.exe) is in stop; it must be a program that may be frozen") }
    if ($large -and $trigger -and $large.exe -ieq $trigger.exe) { $errors.Add('large and trigger must be different programs') }
    $elevated = & $norm $j.elevated 'elevated'
    $b = & $builtinOf $j.tray_program 'tray_program' 'tray_program'
    $tray = if ($null -ne $b) { $b[0] } else { & $norm $j.tray_program 'tray_program' }
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

# Compiles the test programs with the C# compiler of the .NET Framework that
# ships with Windows (no SDK needed) and copies the result to the names the
# lists use. Called when the lists name any of them.
function New-BuiltinPrograms([string]$Dir, $Programs) {
    $wanted = @(@($Programs.suspend) + @($Programs.stop) + @($Programs.large) + @($Programs.tray_program) | Where-Object { $_ -and $_.compiled })
    if (-not $wanted.Count) { return }
    $source = Join-Path $PSScriptRoot $script:BuiltinSource
    if (-not (Test-Path -LiteralPath $source)) { throw "$script:BuiltinSource is missing next to the script ($source); copy the whole scripts folder, or list your own programs in beta-test.json" }
    $csc = @(Join-Path $env:SystemRoot 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'; Join-Path $env:SystemRoot 'Microsoft.NET\Framework\v4.0.30319\csc.exe') | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if (-not $csc) { throw 'the C# compiler of the .NET Framework (csc.exe, v4.0.30319) was not found; list your own programs in beta-test.json instead of the built-in ones' }
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $out = Join-Path $Dir $script:BuiltinExe
    $log = & $csc /nologo /target:winexe /optimize+ /platform:anycpu /reference:System.Windows.Forms.dll /reference:System.Drawing.dll "/out:$out" $source 2>&1
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $out)) { throw "the test programs did not compile (csc exit code $LASTEXITCODE):`n  $(($log | ForEach-Object { "$_" }) -join "`n  ")" }
    foreach ($p in $wanted) { Copy-Item -LiteralPath $out -Destination (Join-Path $Dir $p.exe) -Force }
    Remove-Item -LiteralPath $out -Force
    Write-Good "  $($wanted.Count) test program(s) built: $(($wanted | ForEach-Object { $_.exe }) -join ', ')"
}

function New-SampleFiles([string]$Dir) {
    $drop = Join-Path $Dir 'dragdrop'
    New-Item -ItemType Directory -Force -Path $drop, (Join-Path $Dir 'missing'), (Join-Path $Dir 'beats'), (Join-Path $drop 'a folder') | Out-Null
    $ping = Join-Path $env:SystemRoot 'System32\PING.EXE'
    foreach ($n in 'ks-beta-drop1.exe', 'ks-beta-drop2.exe', 'ks-beta-drop3.exe', 'anydesk.exe') { Copy-Item -LiteralPath $ping -Destination (Join-Path $drop $n) }
    Copy-Item -LiteralPath $ping -Destination (Join-Path $Dir $script:BuiltinTrigger)
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

# The machine's own architecture, also from an emulated (x64 on ARM64) or a
# 32-bit PowerShell: IsWow64Process2, else the environment of the process.
function Get-Arch {
    $a = switch ([KsBetaV3.Native]::NativeMachine()) { 0x8664 { 'AMD64' } 0xAA64 { 'ARM64' } 0x014C { 'x86' } default { $null } }
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

# Variables set for the user or the machine reach every kaltstellen started
# from Explorer or at sign-in, also those the script does not start.
function Show-EnvWarning {
    foreach ($n in $script:EnvNames + @('KALTSTELLEN_UPDATE_URL', 'KALTSTELLEN_TRIGGER_POLL_MS', 'KALTSTELLEN_TRIGGER_DEBOUNCE_MS')) {
        foreach ($scope in 'User', 'Machine') {
            $v = [Environment]::GetEnvironmentVariable($n, $scope)
            if ($v) { Write-Note "Warning: $n is set for the $($scope.ToLowerInvariant()) ($v); kaltstellen started outside this script uses it." }
        }
    }
}

# The kaltstellen that setup quit, and how to start it again.
function Show-QuitBefore {
    foreach ($q in Get-List $script:State.quit_before) {
        Write-Note "  Setup quit your kaltstellen ($($q.path)). After teardown start it again with:"
        Write-Note "    & '$($q.path)' tray"
    }
}

# Puts the copy of an executable that setup replaced back, after checking it
# against the SHA-256 recorded then. Throws if the copy is missing or changed:
# then nothing is replaced or deleted.
function Restore-SavedExe($Entry, [string]$SessionDir, [string]$InstallDir) {
    $saved = Join-Path $SessionDir "backup\install\$($Entry.name)"
    if (-not (Test-Path -LiteralPath $saved)) { throw "the saved $($Entry.name) is missing ($saved); $InstallDir was left as it is" }
    $hash = Get-Sha256 $saved
    if ($hash -ne $Entry.sha256) { throw "the saved $($Entry.name) changed since setup (sha256 $hash, recorded $($Entry.sha256)); $InstallDir was left as it is" }
    Copy-Item -LiteralPath $saved -Destination (Join-Path $InstallDir $Entry.name) -Force
    "put back the earlier $($Entry.name) in $InstallDir (sha256 checked)"
}

# Setup failed before it was ready: the executable that was there before
# goes back in place, after the trays started from the new one have quit.
function Undo-Install {
    $i = $script:State.install
    if (-not $i -or -not $script:State.exe_replaced) { return }
    $pre = @(Get-List $i.preexisting)
    if (-not $pre.Count) { return }
    foreach ($t in Get-TrayWindows) { $null = Stop-TrayWindow $t 15 }
    foreach ($p in Get-OurProcesses) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 500
    foreach ($e in $pre) { Write-Note "  $(Restore-SavedExe $e $script:State.session_dir $script:State.install_dir)" }
    Set-State 'exe_replaced' $false
}

function Invoke-Setup {
    $existing = Read-State
    if ($existing) {
        $how = if ($existing.phase -eq 'restored') { "'beta-test.ps1 teardown -Uninstall' (puts your own kaltstellen.exe back) or 'teardown -KeepBeta'" } else { "'beta-test.ps1 teardown'" }
        throw "a beta test is already set up (phase '$($existing.phase)', $($existing.session_dir)); run $how first"
    }
    # Unique also for two setups in the same second (tests).
    $stamp = (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + ([guid]::NewGuid().ToString('N').Substring(0, 4))
    $arch = Get-Arch
    Show-EnvWarning
    # Long names: %TEMP% may be an 8.3 path, which process paths never are.
    $tempRoot = Join-Path (Get-LongPath ([IO.Path]::GetTempPath())) "kaltstellen-beta-$stamp"
    $sessionDir = Join-Path $script:BackupRoot $stamp
    $realConfig = Get-LongPath (Join-Path $env:APPDATA 'kaltstellen')
    $realData = Get-LongPath (Join-Path $env:LOCALAPPDATA 'kaltstellen')
    $install = if ($InstallDir) { Get-LongPath $InstallDir }
               elseif ($Sandbox) { Join-Path $tempRoot 'bin' }
               else { Get-LongPath (Join-Path $env:LOCALAPPDATA 'Programs\kaltstellen') }
    if ($Sandbox -and $InstallDir) {
        # A sandbox must not replace (or later remove) a real installation.
        $realInstall = Get-LongPath (Join-Path $env:LOCALAPPDATA 'Programs\kaltstellen')
        if ($install -ieq $realInstall) { throw "-Sandbox with -InstallDir $install`: that is the real install location; leave -InstallDir out (the sandbox has its own)" }
        if (Test-Path -LiteralPath (Join-Path $install 'kaltstellen.exe')) { throw "-Sandbox with -InstallDir $install`: it holds a kaltstellen.exe already; use an empty folder or leave -InstallDir out" }
    }
    if ($install -match '(?i)\\Program Files( \(x86\))?\\|\\scoop\\apps\\') { Write-Note "Note: the updater refuses to replace a copy in $install." }
    $configDir = if ($Sandbox) { Join-Path $tempRoot 'config' } else { $realConfig }
    $dataDir = if ($Sandbox) { Join-Path $tempRoot 'data' } else { $realData }
    $regRoot = "Software\kaltstellen-beta-$stamp"
    $source = if ($Exe) { [IO.Path]::GetFullPath($Exe) } else { "https://github.com/$($script:Repo)/releases/download/v$Version/kaltstellen-$Version-windows-$arch.exe" }
    if ($Exe -and -not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "-Exe: file not found: $source" }

    # The program list first: nothing changes while it is not usable. Without
    # a beta-test.json (and without -ProgramList) the test programs the script
    # brings are used.
    $list = if ($ProgramList) { [IO.Path]::GetFullPath($ProgramList) } else { Join-Path $script:BackupRoot 'beta-test.json' }
    if ($ProgramList -and -not (Test-Path -LiteralPath $list)) { throw "-ProgramList: file not found: $list" }
    $useList = Test-Path -LiteralPath $list
    $listFile = if ($useList) { $list } else { '' }
    try { $programs = Read-ProgramList -Path $listFile -TestDir (Join-Path $tempRoot 'test') }
    catch {
        if ($useList -and -not $ProgramList -and $_.Exception.Message -match 'placeholder') { throw "$($_.Exception.Message)`n  Delete $list to use the test programs the script brings, or list your own programs there." }
        throw
    }

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
    Write-Info "  program list:    $(if ($useList) { $list } else { 'none (beta-test.json is optional): the test programs the script brings' })"
    if ($programs) {
        Write-Info "    suspend: $(($programs.suspend | ForEach-Object { $_.exe }) -join ', ')"
        Write-Info "    stop:    $(if ($programs.stop.Count) { ($programs.stop | ForEach-Object { $_.exe }) -join ', ' } else { '(none)' })"
        Write-Info "    trigger: $(if ($programs.trigger) { $programs.trigger.exe } else { '(none)' }); large: $(if ($programs.large) { $programs.large.exe } else { '(none)' }); elevated: $(if ($programs.elevated) { $programs.elevated.exe } else { '(none)' })"
    }
    if ($running.Count) { Write-Note "  running kaltstellen processes: $(($running | ForEach-Object { "$($_.Id) $($_.Path)" }) -join '; ')" }
    if ($DryRun) { Write-Info ''; Write-Info '[dry run] nothing was changed.'; return }

    $quitBefore = @()
    if ($running.Count -and -not $Sandbox) {
        $quitBefore = @(foreach ($r in $running) {
            $cim = Get-CimInstance Win32_Process -Filter "ProcessId = $($r.Id)" -ErrorAction SilentlyContinue
            [pscustomobject]@{ path = $r.Path; command_line = $(if ($cim) { $cim.CommandLine }) }
        })
        Write-Note 'kaltstellen runs already. Its configuration is about to be replaced, so it has to quit first.'
        Write-Note 'Quit resumes frozen programs; stopped programs stay stopped (switch to Neutral first if they should come back).'
        if (-not (Confirm-Yes 'Quit the running kaltstellen now?')) { throw 'setup cancelled: kaltstellen is still running' }
        foreach ($w in @([KsBetaV3.Native]::Windows() | Where-Object { $_.Class -eq 'kaltstellen-tray' -or $_.Class -eq 'kaltstellen-main' })) {
            if ($running.Id -contains [int]$w.Pid) { $null = [KsBetaV3.Native]::PostMessage($w.Handle, $script:WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero) }
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
        started = @(); helper_pids = @(); pending = $null; version = $null; quit_before = $quitBefore
    }
    New-Item -ItemType Directory -Force -Path $script:BackupRoot | Out-Null
    Save-State
    $script:Session = $sessionDir; $script:KsExe = $script:State.exe
    $script:ConfigDir = $configDir; $script:DataDir = $dataDir; $script:TestDir = $script:State.test_dir
    Set-KsEnvironment
    try {
        # 1. Backup, verified before anything changes of the real configuration, data and Run value.
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
        # 2. The executable.
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
        if (-not ((Get-LongPath $file) -ieq $script:KsExe)) { Copy-Item -LiteralPath $file -Destination $script:KsExe -Force; Set-State 'exe_replaced' $true }
        $v = Invoke-Ks --version
        Set-State 'version' (($v.Output | Select-Object -First 1))
        Write-Info "  installed $($script:KsExe): $($script:State.version)"
        if (-not $Exe -and $script:State.version -ne "kaltstellen $Version") { Write-Note "  expected 'kaltstellen $Version'" }


        # 3. Program list, sample files, test configuration.
        Write-Title 'Test configuration'
        $plist = Join-Path $sessionDir 'programs.json'
        if ($useList) { Copy-Item -LiteralPath $list -Destination $plist } else { Write-Utf8 $plist $script:DefaultProgramList }
        Set-State 'programs_file' $plist
        $script:Programs = $programs
        New-Item -ItemType Directory -Force -Path $script:TestDir | Out-Null
        New-SampleFiles $script:TestDir
        New-BuiltinPrograms $script:TestDir $programs
        if ($Sandbox) { New-Item -ItemType Directory -Force -Path $configDir, $dataDir | Out-Null }
        else {
            if (Test-Path -LiteralPath $configDir) { Clear-Folder $configDir } else { New-Item -ItemType Directory -Path $configDir | Out-Null }
        }
        Write-Utf8 (Join-Path $configDir 'config.toml') (New-TestConfig $programs)
        # Automatic switching starts paused; test 'triggers' switches it on.
        Set-SettingsValue 'triggers_paused' $true
        # Updates: check and ask for the whole test, so nothing installs by
        # itself (teardown puts your own settings back).
        Set-SettingsValue 'update' 'manual'
        $cc = Invoke-Ks check-config
        $cc.Output | ForEach-Object { Write-Info "  $_" }
        if ($cc.Code -ne 0) { throw "the generated configuration is not valid (exit code $($cc.Code)); check the program list" }

        # 4. The tray.
        Write-Title 'Tray'
        if (-not $script:IsUnattended) {
            Write-Note '  The tray asks once: "Start kaltstellen automatically when you sign in to Windows?" Answer Yes (test "signin").'
            if ($Sandbox) { Write-Note '  (sandbox: the answer goes to a test registry key that Windows never runs)' }
        }
        $t = Start-Tray
        Write-Good "  tray running (pid $($t.Pid))"
        Set-State 'phase' 'ready'
    } catch {
        Write-Bad "setup failed: $($_.Exception.Message)"
        try { Undo-Install } catch { Write-Bad "  putting the earlier executable back failed: $($_.Exception.Message)" }
        Write-Bad "Run 'beta-test.ps1 teardown' to undo the rest of what setup did."
        Show-QuitBefore
        throw
    }
    Write-Title 'Ready'
    Show-QuitBefore
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
        'Find the kaltstellen icon in the notification area (maybe in the overflow area): a grey window with mint icicles, sharp at your scaling and on a second monitor.',
        'Hover: the tooltip reads "kaltstellen: neutral".',
        'Right click and left click: the menu lists the beta-* profiles, Neutral (checked), Open kaltstellen..., Quit, and no settings (no Start at sign-in, Automatic switching or Updates: lines; they are in the main window); clicking elsewhere closes it.'
    ) -Check {
        param($since)
        $t = Get-MainTray
        $a = Get-Active
        @{ ok = ([bool]$t -and $a -eq 'neutral'); m = [ordered]@{ tray_pid = $(if ($t) { $t.Pid }); active = $a } }
    } -Ask 'Icon, tooltip and menu as described?'

    $null = Request-Running $script:Programs.suspend
    Invoke-Step tray profile 'Choose a profile from the menu (step 4)' -Freeze -Human -Robot { $null = Invoke-Ks switch --no-wait beta-freeze } -Say @(
        'Choose "beta-freeze" in the tray menu.',
        'Expect: the icon turns violet, the tooltip names the profile, the profile is checked.'
    ) -Check {
        param($since)
        $line = Wait-Log '(menu asks for|command line asks for switch to) "beta-freeze"' $since 10 -Regex
        $null = Wait-Log 'switch to "beta-freeze" finished' $since 30
        $f = Get-FreezeReport $script:Programs.suspend
        @{ ok = ([bool]$line -and (Get-Active) -eq 'beta-freeze' -and $f.all); m = [ordered]@{ menu_line = [bool]$line; frozen = $f.detail } }
    } -Ask 'Icon violet, tooltip names beta-freeze, the profile checked?'
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
        } -Ask 'Orange icon with a warning triangle and "Configuration invalid" in the menu?' -Human -Robot {}
    } finally { Write-Utf8 $cfg $original }
    Invoke-Step tray config-fixed 'Configuration fixed again' -Human -Robot {} -Say @('The file is fixed again. Open the menu: the error is gone, the icon is grey.') -Check {
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

    Invoke-Step tray quit 'Quit (step 9)' -Human -Robot { $t = Get-MainTray; if (Test-KsSupports quit) { $null = Invoke-Ks quit } else { $null = [KsBetaV3.Native]::PostMessage($t.Handle, $script:WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero) } } -Say @('Choose Quit in the tray menu: the icon disappears.') -Check {
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
        'At the first start the tray asked "Start kaltstellen automatically when you sign in to Windows?" (in setup).',
        'If the question is still open: answer Yes. If you answered No, the script switches it on after Enter (kaltstellen autostart on).'
    ) -Check {
        param($since)
        $asked = Find-Log 'asking whether to start at sign-in' $script:State.setup_utc
        $answer = Find-Log 'start at sign-in question answered' $script:State.setup_utc
        $a = Get-AutostartState
        $before = $a.text
        $byScript = $false
        if ($a.state -eq 'other') {
            # The Run value starts another program (an earlier copy): the test
            # points it at the beta; teardown restores the value as it was.
            Write-Note "   Start at sign-in starts another program ($($a.text)). For the test it is set to the beta; teardown puts the value back as it was."
        }
        if ($a.state -ne 'on') {
            # Answered No (or not at all), or another program: on through the
            # command line, as the question's text suggests; T7 needs it.
            $null = Invoke-Ks autostart on
            $a = Get-AutostartState
            $byScript = $true
        }
        $rv = Get-RunValue (Get-RunKey)
        $expected = "`"$exe`" tray"
        @{ ok = ($a.state -eq 'on' -and $rv.present -and $rv.data -ieq $expected)
           m = [ordered]@{ asked_in_log = [bool]$asked; answer = $(if ($answer) { $answer.Substring(24).Trim() }); switched_on_by_script = $byScript; autostart_before = $before; autostart = $a.text; run_key = "HKCU\$(Get-RunKey)"; run_value = $rv.data } }
    } -Ask 'The tray icon and its menu worked while the question was open, and the text names "Start at sign-in" in the main window and "kaltstellen autostart on|off"?'
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
    foreach ($w in Get-GuiWindows) { $null = [KsBetaV3.Native]::PostMessage($w.Handle, $script:WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero) }
    $null = Wait-Until 15 { -not (Get-GuiWindows).Count }
}

function Get-WebViewProcesses {
    $marker = (Join-Path $script:DataDir 'webview2')
    @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue | Where-Object { $_.CommandLine -and $_.CommandLine.IndexOf($marker, [StringComparison]::OrdinalIgnoreCase) -ge 0 } | ForEach-Object { [int]$_.ProcessId })
}

# Step 11: the two switches in the header of the main window follow the
# command line, and "Automatic switching" is disabled with a hint while no
# profile has triggers.
# "kaltstellen autostart": on (this executable), off, or other (the Run value
# starts another program; "on, but for another program: ..." is not on).
function Get-AutostartState {
    $r = Invoke-Ks autostart
    $line = @($r.Output | Where-Object { $_ -match '^start at sign-in:' }) | Select-Object -First 1
    $state = if ($line -match '^start at sign-in:\s*on\s*$') { 'on' }
             elseif ($line -match '^start at sign-in:\s*off\s*$') { 'off' }
             elseif ($line -match '^start at sign-in:\s*on, but for another program') { 'other' }
             else { 'unknown' }
    [pscustomobject]@{ state = $state; text = $r.Text }
}

function Test-WindowHeader {
    $null = Open-Gui
    $before = Get-AutostartState
    $wasOn = $before.state -eq 'on'
    # The Run value exactly as it is now goes back after the step, whatever happens.
    $savedRun = Get-RunValue (Get-RunKey)
    try {
    Invoke-Step window settings-autostart '"Start at sign-in" in Settings, General, follows the command line (step 11)' -Prepare {
        $r = Invoke-Ks autostart $(if ($wasOn) { 'off' } else { 'on' })
        [ordered]@{ autostart_before = $before.text; switched = $r.Text }
    } -Say @(
        "The script switched start at sign-in $(if ($wasOn) { 'off' } else { 'on' }) with ""kaltstellen autostart"". Open Settings (sidebar foot), General, and click into the window: the switch ""Start at sign-in"" follows."
    ) -Check {
        param($since)
        $now = Get-AutostartState
        $null = Restore-RunValue $savedRun (Get-RunKey)
        $after = Get-RunValue (Get-RunKey)
        $restored = ($after.present -eq $savedRun.present) -and ($after.data -eq $savedRun.data)
        @{ ok = ($now.state -eq $(if ($wasOn) { 'off' } else { 'on' }) -and $restored); m = [ordered]@{ during = $now.text; restored = $restored } }
    } -Ask 'The switch in Settings followed (it is back as before now; click into the window again)?'
    } finally { $null = Restore-RunValue $savedRun (Get-RunKey) }

    if (-not (Test-KsSupports automation)) {
        Add-Skip window header-automation 'the installed build has no "kaltstellen automation" (no header switch)' 'n/a'
        return
    }
    Invoke-Step window header-automation '"Automatic switching" in the header (step 11)' -Check {
        param($since)
        $a = Invoke-Ks automation
        $noTriggers = $a.Text -match 'no profile has triggers'
        @{ ok = ($a.Code -eq 0 -and $a.Text -match 'automatic switching: paused' -and ($noTriggers -eq (-not $script:Programs.trigger))); m = [ordered]@{ automation = $a.Text } }
    } -Say @('The test keeps automatic switching paused until test "triggers": in the header "Automatic switching" is enabled and unchecked.') -Ask 'Enabled and unchecked?'

    $cfg = Join-Path $script:ConfigDir 'config.toml'
    $original = [IO.File]::ReadAllText($cfg)
    try {
        Invoke-Step window header-hint '"Automatic switching" disabled without triggers (step 11)' -Human -Robot {} -Prepare {
            $noTrigger = $script:Programs.PSObject.Copy()
            $noTrigger.trigger = $null
            Write-Utf8 $cfg (New-TestConfig $noTrigger)
            $null
        } -Say @(
            'The script wrote the test configuration without the trigger profile. Click into the window:',
            '"Automatic switching" is disabled; hovering it shows why (no profile has triggers).'
        ) -Check {
            param($since)
            $a = Invoke-Ks automation
            @{ ok = ($a.Text -match 'no profile has triggers'); m = [ordered]@{ automation = $a.Text } }
        } -Ask 'Disabled, and the hint says why?'
    } finally { Write-Utf8 $cfg $original }
}

# The Settings view (sidebar foot): every setting of the tray menu and the
# files in one place. The script changes nothing but what a step names and
# puts it back afterwards.
function Test-WindowSettings {
    $null = Open-Gui
    $cfg = Join-Path $script:ConfigDir 'config.toml'
    $settingsFile = Join-Path $script:ConfigDir 'settings.json'
    Invoke-Step window settings-view 'Settings view: sections and scrolling (13a)' -Human -Say @(
        'Choose "Settings" at the foot of the sidebar: the page shows General, Defaults, Profiles, Updates, Files and About.',
        'Click each section in the list on the left: the page scrolls to it without jumping; About, the last one, can be scrolled back up with the mouse wheel.',
        'Profiles lists the beta-* profiles and a click opens one; the header still shows "Automatic switching".'
    ) -Ask 'Sections, scrolling and the profile list as described?' -AskNote

    $original = [IO.File]::ReadAllText($cfg)
    try {
        Invoke-Step window settings-defaults 'Defaults are saved into config.toml (13a)' -Human -Say @(
            'Settings, Defaults: set "Stop timeout" to 12 seconds and press Save: the status line says it was saved.',
            'An out-of-range value (0, or more than 300) is refused with a reason and nothing is written.'
        ) -Check {
            param($since)
            $text = [IO.File]::ReadAllText($cfg)
            $cc = Invoke-Ks check-config
            $saved = $text -match '(?m)^\s*stop_timeout_s\s*=\s*12\b'
            @{ ok = ($saved -and $text.Contains('# beta-test: generated') -and $cc.Code -eq 0); m = [ordered]@{ stop_timeout_12 = $saved; comments_kept = $text.Contains('# beta-test: generated'); check_config_exit = $cc.Code } }
        } -Ask 'Saved, the range check works?'
    } finally { Write-Utf8 $cfg $original }

    $updateBefore = (Read-JsonFile $settingsFile).update
    try {
        Invoke-Step window settings-updates 'Updates section: version and mode (13a)' -Human -Say @(
            'Settings, Updates: the installed version is shown and the mode reads "Check and ask" (set for the test). Choose "Off".',
            '"Check now" contacts GitHub (your choice): "Checking...", then the result. It is not needed for this step.'
        ) -Check {
            param($since)
            $v = (Read-JsonFile $settingsFile).update
            @{ ok = ($v -eq 'off'); m = [ordered]@{ update_in_settings_json = "$v" } }
        } -Ask 'Version, mode and "Check now" as described?'
    } finally { Set-SettingsValue 'update' $(if ($updateBefore) { $updateBefore } else { 'manual' }) }

    Invoke-Step window settings-files 'Files section and "Forget last profile" (13a)' -Human -Freeze -Prepare {
        $null = Request-Running $script:Programs.suspend
        $null = Switch-Profile 'beta-freeze'
        $null = Switch-Profile neutral
        $script:Ctx.last_profile = (Read-JsonFile $settingsFile).last_profile
        [ordered]@{ last_profile_before = "$($script:Ctx.last_profile)" }
    } -Say @(
        'Settings, Files: the three paths are those of the test (the temporary folder in a sandbox). "Open config folder" and "Open log file" open Explorer / the editor; close them again.',
        '"Reload" says the file was read again. "Forget last profile": press it; the status line confirms.'
    ) -Check {
        param($since)
        $now = (Read-JsonFile $settingsFile).last_profile
        @{ ok = $(if ($script:Ctx.last_profile) { -not $now } else { $null }); m = [ordered]@{ last_profile_after = "$now" } }
    } -Ask 'Paths, Open, Reload and Forget as described?' -AskNote
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
        'Select "beta-freeze" in the window and press "Apply profile": the button shows "Applying...", then the status line the result; the tray icon turns violet.'
    ) -Check {
        param($since)
        $line = Wait-Log 'switch to "beta-freeze"' $since 10
        $null = Wait-Log 'switch to "beta-freeze" finished' $since 30
        $f = Get-FreezeReport $script:Programs.suspend
        @{ ok = ([bool]$line -and $f.all); m = [ordered]@{ frozen = $f.detail } }
    } -Ask 'Button, status line and tray icon as described?'
    Set-Neutral

    Test-WindowHeader

    Invoke-Step window misc 'Context menu (step 13)' -Human -Say @(
        'Right click in the page: no browser context menu; Ctrl+P, F5, Ctrl+F do nothing.'
    ) -Ask 'All as described?' -AskNote

    Test-WindowSettings

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
        # Only the copy in the test folder (started by an earlier run of this test).
        Get-ProgramProcesses 'ks-beta-missing.exe' | Where-Object { $_.Path -and (Get-LongPath $_.Path) -ieq $missing } | Stop-Process -Force -ErrorAction SilentlyContinue
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
        'Tray: the icon is orange with a warning triangle, the menu shows the error line; opening and closing the menu changes nothing.',
        'Window, profile beta-failure: the start entry has a red mark "KS110 start failed", hovering shows the whole error, the row has a red edge; other entries and profiles show nothing.'
    ) -Check {
        param($since)
        $r = $script:Ctx.failure
        $f = Get-FreezeReport @($first)
        @{ ok = ($r.Code -eq 1 -and $r.Text -match 'KS110' -and $f.all); m = [ordered]@{ frozen = $f.detail } }
    } -Ask 'Orange icon with a warning triangle, error line in the menu, marker in the window as described?' -AskNote

    $resolve = [pscustomobject]@{ exe = 'ks-beta-missing.exe'; path = $missing; args = @('-n', '600', '127.0.0.1'); disposable = $true; hidden = $true }
    Invoke-Step failure resolves 'The error resolves by itself' -Prepare {
        Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\PING.EXE') -Destination $missing
        $script:Ctx.resolve_since = Get-UtcStamp (-200)
        $null = Start-Program $resolve -Hidden
        [ordered]@{ started = 'ks-beta-missing.exe (a copy of ping.exe) at the missing path' }
    } -Say @(
        'The script put a program at that path and started it.',
        'Within about 30 s (or when the menu opens) the tray icon turns violet again without a switch.',
        'Click into another window and back into kaltstellen: the marker is gone.'
    ) -Check {
        param($since)
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $line = Wait-Log 'tray: error resolved by itself' $script:Ctx.resolve_since 45
        @{ ok = [bool]$line; m = [ordered]@{ resolved_line = $(if ($line) { $line.Substring(24).Trim() }); waited_ms = $sw.ElapsedMilliseconds } }
    } -Ask 'Icon violet again and the marker gone?'

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
    # "kaltstellen list --info" shows description, product and publisher; a
    # build without it has no program descriptions at all.
    if (-not (Test-KsSupports list '--info')) {
        Add-Skip descriptions show 'the installed build has no "kaltstellen list --info" (no program descriptions)' 'n/a'
        return
    }
    $null = Start-Tray
    $running = Request-Running $script:Programs.suspend
    Invoke-Step descriptions show 'Program descriptions (beta-test 1.4)' -Prepare {
        $m = [ordered]@{}
        foreach ($p in $running) { $m["list --info $($p.exe)"] = ((Invoke-Ks list --info ([IO.Path]::GetFileNameWithoutExtension($p.exe))).Output | Select-Object -First 8) -join ' | ' }
        $m['status --unassigned'] = ((Invoke-Ks status --unassigned).Output | Select-Object -First 12) -join ' | '
        $null = Open-Gui
        $m
    } -Say @(
        'In the window: the description, product and publisher of your programs (profile beta-freeze, "Add program...", the unassigned list).',
        'Also look at the output of "kaltstellen list --info" and "kaltstellen status --unassigned" (recorded in the results).'
    ) -Check {
        param($since)
        # The command works for each running program.
        $out = @($running | ForEach-Object { (Invoke-Ks list --info ([IO.Path]::GetFileNameWithoutExtension($_.exe))) })
        $ok = -not @($out | Where-Object { $_.Code -ne 0 }).Count
        @{ ok = $ok; m = [ordered]@{ list_info_exit_codes = @($out | ForEach-Object { $_.Code }) } }
    } -Ask 'Descriptions, products and publishers shown and right?' -AskNote
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
    $record = [pscustomobject]@{ pid = $p.Id; start_ticks = (Get-StartTicks $p) }
    Set-State 'helper_pids' (@(Get-List $script:State.helper_pids) + @($record))
    $null = Wait-Until 15 { @(Get-TrayWindows | Where-Object { [int]$_.Pid -eq $p.Id }).Count }
    Start-Sleep -Milliseconds 500
    $p
}

# Quits the helper trays: only a process that is still the one started (same
# start time) from the installed executable.
function Stop-HelperTrays {
    foreach ($h in Get-List $script:State.helper_pids) {
        if (-not $h.pid) { continue }
        $p = Get-RecordedProcess $h.pid $h.start_ticks $script:KsExe
        if (-not $p) { continue }
        $w = Get-TrayWindows | Where-Object { [int]$_.Pid -eq $p.Id } | Select-Object -First 1
        if ($w) { $null = Stop-TrayWindow $w 10 } else { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    }
    Set-State 'helper_pids' @()
}

# Unattended: one short press, then one held down (the key repeats).
function Send-Hotkey([string]$Hotkey) {
    $vk = ConvertTo-VirtualKeys $Hotkey
    if (-not $vk) { throw "cannot type $Hotkey" }
    $null = [KsBetaV3.Keys]::Chord($vk.mods, $vk.key, 1)
    Start-Sleep -Milliseconds 700
    $null = [KsBetaV3.Keys]::Chord($vk.mods, $vk.key, 15)
    Start-Sleep -Milliseconds 700
}

function Test-Hotkeys {
    $hk = $script:Programs.hotkey
    $null = Start-Tray
    Set-Neutral
    $null = Request-Running $script:Programs.suspend
    Invoke-Step hotkeys press "A real key press (manual-checks ""Hotkeys"" 1, T17)" -Freeze -Human -Robot { Send-Hotkey $hk } -Say @(
        "Press $hk once briefly. Then press it a second time and hold it for 2 s.",
        'Expect: "beta-freeze" is applied (icon violet), once per press even when held down.'
    ) -Check {
        param($since)
        Start-Sleep -Milliseconds 800
        $hits = Find-Log 'hotkey asks for "beta-freeze"' $since -All
        $f = Get-FreezeReport $script:Programs.suspend
        @{ ok = ($hits.Count -eq 2 -and (Get-Active) -eq 'beta-freeze'); m = [ordered]@{ hotkey_requests = $hits.Count; frozen = $f.detail } }
    } -Ask 'The icon turned violet on the first press?'
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
        $listed = { $r = Invoke-Ks status --unassigned; [bool]($r.Output | Where-Object { $_ -match [regex]::Escape($tp.exe) -and $_ -match '\(new\)' }) }
        $ok = $null
        # The tray icon of a new program takes a while to be noticed (longer on ARM64).
        if ($tp) { $ok = [bool](Wait-Until 40 $listed 1000) }
        $u = Invoke-Ks status --unassigned
        @{ ok = $ok; m = [ordered]@{ unassigned = (($u.Output | Select-Object -First 10) -join ' | ') } }
    } -Ask 'Menu item, "new" marker, "Add to profile" and "Dismiss" as described?' -AskNote
    Set-Neutral
    Close-Gui
}

# The test programs the script brings: they run, beat, answer, close or
# refuse to close as designed. Everything else in the run builds on this.
function Test-Programs {
    $compiled = @(@($script:Programs.suspend) + @($script:Programs.stop) + @($script:Programs.large) + @($script:Programs.tray_program) | Where-Object { $_ -and $_.compiled })
    if (-not $compiled.Count) { Add-Skip programs start 'the program list names no built-in test programs'; return }
    $null = Start-Tray
    Set-Neutral
    foreach ($p in $compiled) { $null = Close-Program $p -Wait }
    Invoke-Step programs start 'The test programs start, beat and answer' -Prepare {
        $null = Request-Running $compiled 2000
        $m = [ordered]@{}
        foreach ($p in $compiled) { $m[$p.exe] = $(if ((Get-ProgramProcesses $p.exe).Count) { 'running' } else { 'not running' }) }
        $m
    } -Check {
        param($since)
        $m = [ordered]@{}; $ok = $true
        foreach ($p in $compiled) {
            $b0 = Get-Beat $p
            $moved = [bool](Wait-Until 5 { (Get-Beat $p) -ne $b0 } 100)
            $answer = $null
            if ($p.role -ne 'tray_program') {
                # The window appears a moment after the process (slower on ARM64).
                # A box: the condition runs in a scope of its own.
                $box = @{ answer = -1 }
                $null = Wait-Until 20 {
                    $w = Get-MainWindows $p | Select-Object -First 1
                    if ($w) { $box.answer = [KsBetaV3.Native]::Ping($w.Handle, 2000) }
                    $box.answer -ge 0
                } 300
                $answer = $box.answer
            }
            $m[$p.exe] = "beat $(if ($moved) { 'rises' } else { 'does not move' })$(if ($null -ne $answer) { ", window answers in $answer ms" })"
            if (-not $moved -or ($null -ne $answer -and $answer -lt 0)) { $ok = $false }
        }
        @{ ok = $ok; m = $m }
    }
    $closing = @($compiled | Where-Object { $_.behavior })
    if ($closing.Count) {
        Invoke-Step programs close 'A close request closes the program, except the stubborn ones' -Prepare {
            foreach ($p in $closing) { foreach ($q in Get-StartedProcesses $p) { $null = $q.CloseMainWindow() } }
            Start-Sleep -Seconds 3
            $null
        } -Check {
            param($since)
            $m = [ordered]@{}; $ok = $true
            foreach ($p in $closing) {
                $alive = (Get-ProgramProcesses $p.exe).Count -gt 0
                $m[$p.exe] = $(if ($alive) { 'stays' } else { 'closed' })
                if ($alive -ne ($p.behavior -eq 'stays' -or $p.behavior -eq 'killed')) { $ok = $false }
            }
            @{ ok = $ok; m = $m }
        }
    }
    foreach ($p in $compiled) { $null = Close-Program $p -Wait }
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
        $null = Wait-Log 'switch to "beta-stop" finished' $since 30
        foreach ($p in $running) {
            $alive = (Get-ProgramProcesses $p.exe).Count
            $m[$p.exe] = $(if ($alive) { 'still running' } else { 'closed' })
            # The test programs have a known behaviour; programs of your own
            # may stay open only with on_timeout = "keep".
            if ($p.behavior -in 'closes', 'killed') { if ($alive) { $ok = $false } }
            elseif ($p.behavior -eq 'stays') { if (-not $alive) { $ok = $false } }
            elseif ($alive -and $p.on_timeout -ne 'keep') { $ok = $false }
        }
        $m.status = ((Invoke-Ks status).Output -join ' | ')
        $stayed = @($m.Values | Where-Object { $_ -eq 'still running' }).Count
        @{ ok = $ok; m = $m; note = $(if ($stayed) { 'with on_timeout = "keep" a program that asks to save stays open and is reported (KS108)' }) }
    } -Ask 'The programs closed (or asked to save and stayed, reported)?'
    Invoke-Step stop restart 'Neutral starts them again (restart_on_neutral)' -Prepare {
        $script:Ctx.restart_from = [DateTime]::UtcNow.AddSeconds(-1)
        $r = Switch-Profile neutral
        [ordered]@{ exit = $r.Code; ms = $r.Ms }
    } -Check {
        param($since)
        Start-Sleep -Seconds 3
        $m = [ordered]@{}; $ok = $true
        foreach ($p in $running) {
            $procs = Get-ProgramProcesses $p.exe
            $alive = $procs.Count -gt 0; $m[$p.exe] = $alive; if ($p.restart -and -not $alive) { $ok = $false }
            # A test fixture that kaltstellen started again for this test is
            # ended by teardown like one the script started.
            if ($p.disposable) {
                foreach ($q in $procs) { try { if ($q.StartTime.ToUniversalTime() -ge $script:Ctx.restart_from) { Register-Started $q $p } } catch {} }
            }
        }
        @{ ok = $ok; m = $m }
    } -Ask 'The programs came back and work?'
}

# Automatic switching is on only while the triggers test runs: paused again
# also after q, Ctrl+C or an error.
function Test-Triggers {
    try { Test-TriggersSteps }
    finally {
        try {
            # Without a trigger the test ran no step and switched nothing on.
            if ($script:Programs.trigger) {
                $null = Close-Program $script:Programs.trigger -Wait
                Set-TriggersPaused $true
            }
        } catch { Write-Bad "   could not pause automatic switching again: $($_.Exception.Message)" }
    }
}

function Test-TriggersSteps {
    $tr = $script:Programs.trigger
    if (-not $tr) { Add-Skip triggers all 'the program list has no trigger'; return }
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
    $hasAutomation = Test-KsSupports automation
    Invoke-Step triggers header 'Automatic switching in the main window (step 2)' -Prepare { Open-Gui } -Say @(
        'Click into the main window: "Automatic switching" in the header is enabled and checked. The tray menu has no such item.'
    ) -Check {
        param($since)
        if (-not $hasAutomation) { return @{ ok = $null; m = [ordered]@{ automation = 'the installed build has no kaltstellen automation' } } }
        $a = Invoke-Ks automation
        @{ ok = ($a.Text -match 'automatic switching: on'); m = [ordered]@{ automation = $a.Text } }
    } -Ask 'Enabled and checked, and no such item in the tray menu?'

    $startTrigger = {
        if ($tr.path) { $script:Ctx.trigger_started = Get-UtcStamp; $null = Start-Program $tr; return [ordered]@{ started_by = 'script' } }
        Write-Info "   Start $($tr.exe) now."
        [ordered]@{ started_by = 'person' }
    }
    # The tray looks for triggers every 5 s: after closing the trigger, wait
    # until it has seen the end before starting it again.
    # The tray takes a trigger for ended at a poll at least 5 s after it last
    # saw it, and polls every 5 s: up to about 10 s. Wait for its own line.
    # The tray's lines about the trigger: names as Windows reports them (case
    # may differ), so case-insensitive and escaped for both lookups.
    $triggerLine = '(?i)tray: trigger ' + [regex]::Escape($tr.exe) + ' of "beta-trigger" '
    $closeTrigger = {
        # Only an end the tray can see: it reports one for a trigger it saw start.
        $seen = Find-Log ($triggerLine + '(started|ended)') $script:State.setup_utc -Regex -All
        $last = @($seen) | Select-Object -Last 1
        $wasRunning = (Get-ProgramProcesses $tr.exe).Count -gt 0 -and $last -and $last -match 'started \(priority'
        $from = Get-UtcStamp
        $null = Request-Closed $tr
        if ($tr.builtin) {
            # The built-in trigger is the script's own copy in the test folder.
            foreach ($q in Get-ProgramProcesses $tr.exe | Where-Object { $_.Path -and (Test-Below (Get-LongPath $_.Path) $script:TestDir) }) {
                Write-Note "   $($tr.exe) (pid $($q.Id)) from the test folder still ran; ending it"
                Stop-Process -Id $q.Id -Force -ErrorAction SilentlyContinue
            }
        }
        $script:Ctx.trigger_end_seen = if ($wasRunning) {
            [bool](Wait-Log ($triggerLine + 'ended') $from 15 -Regex)
        } else { $null }
        if ($script:Ctx.trigger_end_seen -eq $false) { Write-Note "   the tray did not report the end of $($tr.exe) within 15 s" }
    }
    Invoke-Step triggers applies 'The trigger applies its profile (step 3)' -Freeze -Prepare $startTrigger -Say @(
        "Within about 5 s after $($tr.exe) started: icon violet, tooltip ends in ""(by trigger)"", a notification ""kaltstellen: beta-trigger"" appears;",
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
            $applied = [bool](Wait-Log 'switch to "beta-trigger" finished' $s 25)
            $s = Get-UtcStamp
            $null = Request-Closed $tr
            $back = [bool](Wait-Log 'trigger asks for "neutral"' $s 25)
            $null = Start-Program $tr
            return [ordered]@{ driven_by = 'script'; trigger_end_seen = $script:Ctx.trigger_end_seen; first_applied = $applied; back_to_neutral = $back }
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
        $script:Ctx.applied = [bool](Wait-Log 'switch to "beta-trigger" finished' $s 25)
        $active = Get-Active
        $script:Ctx.closed_at = Get-UtcStamp
        $null = Request-Closed $tr
        [ordered]@{ trigger_end_seen = $script:Ctx.trigger_end_seen; applied_by_trigger = $script:Ctx.applied; active_before_close = $active }
    } -Check {
        param($since)
        if (-not $script:Ctx.applied) { return @{ ok = $false; note = 'the trigger did not apply beta-trigger within 25 s, so its end could not be tested' } }
        $back = Wait-Log 'trigger asks for "neutral"' $script:Ctx.closed_at 25
        $null = Wait-Log 'switch to "neutral" finished' $script:Ctx.closed_at 25
        $ms = if ($back) { [int]((Get-LogTime $back) - (ConvertFrom-UtcStamp $script:Ctx.closed_at)).TotalMilliseconds }
        @{ ok = ([bool]$back -and (Get-Active) -eq 'neutral'); m = [ordered]@{ back_after_ms = $ms } }
    } -Ask 'A notification said so when neutral came back?'

    Invoke-Step triggers manual-wins 'A switch of your own wins (step 6)' -Freeze -Prepare {
        & $closeTrigger
        $s = Get-UtcStamp
        $null = & $startTrigger
        $script:Ctx.applied = [bool](Wait-Log 'switch to "beta-trigger" finished' $s 25)
        $r = Switch-Profile 'beta-freeze'
        $script:Ctx.closed_at = Get-UtcStamp
        $null = Request-Closed $tr
        [ordered]@{ trigger_end_seen = $script:Ctx.trigger_end_seen; applied_by_trigger = $script:Ctx.applied; manual_switch_exit = $r.Code }
    } -Check {
        param($since)
        if (-not $script:Ctx.applied) { return @{ ok = $false; note = 'the trigger did not apply beta-trigger within 25 s first' } }
        Start-Sleep -Seconds 15
        $a = Get-Active
        @{ ok = ($a -eq 'beta-freeze'); m = [ordered]@{ active_15s_after_trigger_ended = $a } }
    }
    Set-Neutral

    if (-not $hasAutomation) {
        Add-Skip triggers pause 'the installed build has no "kaltstellen automation"' 'n/a'
    } else {
        Invoke-Step triggers pause 'Pausing automatic switching (step 7)' -Prepare {
            $script:Ctx.pause_from = Get-UtcStamp
            $r = Invoke-Ks automation off
            [ordered]@{ automation_off = ($r.Output -join ' | ') }
        } -Say @(
            'The script paused automatic switching with "kaltstellen automation off" (as unchecking it in the main window does).',
            'The tray menu shows the disabled line "Automatic switching paused" at once; click into the main window: the header switch is unchecked.'
        ) -Check {
            param($since)
            $line = Wait-Log 'tray: automatic switching paused' $script:Ctx.pause_from 5
            $s = Read-JsonFile (Join-Path $script:ConfigDir 'settings.json')
            $paused = [bool]($s -and $s.triggers_paused)
            $null = & $startTrigger
            Start-Sleep -Seconds 12
            $a = Get-Active
            Stop-Tray
            $t0 = Get-UtcStamp (-200)
            $null = Start-Tray
            $kept = Wait-Log 'automatic switching is paused' $t0 5
            @{ ok = ([bool]$line -and $paused -and $a -eq 'neutral' -and [bool]$kept); m = [ordered]@{ tray_took_it = [bool]$line; settings_paused = $paused; active_with_trigger_running = $a; paused_after_restart = [bool]$kept } }
        } -Ask 'The disabled line in the tray menu, and after the restart of the tray (done by the script) the header switch is still unchecked?'
        Invoke-Step triggers resume 'Switching it on again (step 7)' -Prepare {
            $script:Ctx.resume_from = Get-UtcStamp
            $r = Invoke-Ks automation on
            [ordered]@{ automation_on = ($r.Output -join ' | ') }
        } -Say @('The script switched it on again ("kaltstellen automation on"): the running trigger counts as starting and the profile is applied.') -Check {
            param($since)
            $ask = Wait-Log 'trigger asks for "beta-trigger"' $script:Ctx.resume_from 20
            @{ ok = [bool]$ask }
        } -Ask 'The profile was applied and the header switch is checked again?'
    }
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
        foreach ($p in $running) { $w = Get-MainWindows $p | Select-Object -First 1; if ($w) { $pings[$p.exe] = [KsBetaV3.Native]::Ping($w.Handle, 5000) } }
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
    $tray = [KsBetaV3.Native]::FindWindow('Shell_TrayWnd', $null)
    Invoke-Step t14 shell 'Shell stays responsive with windowed programs frozen (T14)' -Freeze -Prepare {
        $m = [ordered]@{ programs = @($windowed | ForEach-Object { $_.exe }); taskbar_ping_before_ms = [KsBetaV3.Native]::Ping($tray, 3000) }
        $r = Switch-Profile 'beta-freeze'
        $m.switch_exit = $r.Code
        Start-Sleep -Seconds 6
        $m.frozen_windows = @(foreach ($p in $windowed) { foreach ($w in Get-MainWindows $p) { [ordered]@{ exe = $p.exe; iconic = [KsBetaV3.Native]::IsIconic($w.Handle); hung = [KsBetaV3.Native]::IsHungAppWindow($w.Handle) } } })
        $m.ghost_windows = @([KsBetaV3.Native]::Windows() | Where-Object { $_.Class -eq 'Ghost' -and $_.Visible }).Count
        $m.taskbar_ping_frozen_ms = [KsBetaV3.Native]::Ping($tray, 3000)
        $m.explorer_responding = @(Get-Process explorer -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $script:SessionId } | ForEach-Object { $_.Responding })
        $folder = Join-Path $script:TestDir 'dragdrop'
        # Only the window opened here is measured and closed, not one of yours.
        $existing = @([KsBetaV3.Native]::Windows() | Where-Object { $_.Class -eq 'CabinetWClass' } | ForEach-Object { $_.Handle })
        $sw = [Diagnostics.Stopwatch]::StartNew()
        Start-Process explorer.exe -ArgumentList (Quote-Arg $folder)
        $h = Wait-Until 15 { [KsBetaV3.Native]::Windows() | Where-Object { $_.Class -eq 'CabinetWClass' -and $existing -notcontains $_.Handle -and $_.Title -like '*dragdrop*' } | Select-Object -First 1 } 50
        $m.explorer_window_open_ms = if ($h) { $sw.ElapsedMilliseconds } else { -1 }
        if ($h) {
            Start-Sleep -Seconds 1
            $m.explorer_window_ping_ms = [KsBetaV3.Native]::Ping($h.Handle, 3000)
            $null = [KsBetaV3.Native]::PostMessage($h.Handle, $script:WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
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
        foreach ($p in $windowed) { $w = Get-MainWindows $p | Select-Object -First 1; if ($w) { $v = [KsBetaV3.Native]::Ping($w.Handle, 5000); $pings[$p.exe] = $v; if ($v -lt 0) { $ok = $false } } }
        @{ ok = $ok; m = [ordered]@{ window_answer_ms = $pings; ghost_windows = @([KsBetaV3.Native]::Windows() | Where-Object { $_.Class -eq 'Ghost' -and $_.Visible }).Count } }
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
    if ($w) { $m.window_answer_after_neutral_ms = [KsBetaV3.Native]::Ping($w.Handle, 10000) }
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
    if (-not $large) { Add-Skip t19 trim 'the program list has no "large" program'; return }
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
    $uj = Join-Path $script:DataDir 'update.json'
    $had = Test-Path -LiteralPath $uj
    $old = if ($had) { [IO.File]::ReadAllText($uj) } else { $null }
    try {
        Invoke-Step updates offer 'A newer version is offered (manual-checks "Updates" 2)' -Prepare {
            Write-Utf8 $uj '{ "latest": "9.9.9" }'
            $null
        } -Say @(
            'The script wrote update.json naming version 9.9.9 (it does not exist). Open the tray menu:',
            'a release build shows "Install update 9.9.9...", a development build a disabled "Update 9.9.9 available: ..." line. Do NOT click it.'
        ) -Ask 'The menu shows the line as described?'
    } finally {
        if ($had) { Write-Utf8 $uj $old } elseif (Test-Path -LiteralPath $uj) { Remove-Item -LiteralPath $uj -Force }
        # The tray keeps the offer it read until it reads update.json again.
        if (Get-MainTray) { Set-Neutral; Stop-Tray; $null = Start-Tray }
    }

    # The mode through the running tray, as the main window sets it. Off from
    # here on: "check and ask" would check GitHub at once when it is chosen.
    Invoke-Step updates modes 'Update mode through the tray (manual-checks "Updates" 1)' -Prepare {
        $script:Ctx.mode_from = Get-UtcStamp
        $r = Invoke-Ks update --mode off
        [ordered]@{ exit = $r.Code; output = ($r.Output -join ' | ') }
    } -Say @(
        'The script set the update mode to off with "kaltstellen update --mode off" (as the main window''s Updates section does); it stays off for the rest of the test.',
        'Main window, Updates: Mode reads "Off" (click into the window). The tray menu has no mode items.'
    ) -Check {
        param($since)
        $line = Wait-Log 'tray: update mode off' $script:Ctx.mode_from 5
        $s = Read-JsonFile $settings
        $final = if ($s) { $s.update } else { $null }
        @{ ok = ([bool]$line -and $final -eq 'off'); m = [ordered]@{ tray_took_it = [bool]$line; settings_update = $final } }
    } -Ask 'Mode "Off" in the main window, and no mode items in the tray menu?'

    if ($script:IsUnattended -or -not (Confirm-Yes '   Is a newer beta released and do you want to install it now (it is downloaded from GitHub and replaces the installed copy)?')) {
        Add-Skip updates install 'no newer release to install yet (beta-test section 4)'
        return
    }
    $before = (Invoke-Ks --version).Output | Select-Object -First 1
    $oldExe = Join-Path $script:State.install_dir 'kaltstellen.old.exe'
    $oldHash = if (Test-Path -LiteralPath $oldExe) { Get-Sha256 $oldExe } else { $null }
    $null = Request-Running $script:Programs.suspend
    $auto = Confirm-Yes '   Test the automatic install (y) or the install on request with frozen programs (n)?'
    $ks = "& '$($script:KsExe)'"
    Invoke-Step updates install $(if ($auto) { 'Automatic install while neutral (step 1a)' } else { 'Install on request with frozen programs (step 4)' }) -Freeze -Human -Say $(if ($auto) {
            @("Apply beta-freeze. Then set the mode in another terminal ($ks update --mode auto) or in the main window (Mode: Install automatically): it checks at once, and nothing is installed while the profile is active.",
              'Then choose Neutral: the update installs without a window, the tray icon disappears briefly and comes back. Wait for it, then press Enter.')
        } else {
            @('Apply beta-freeze. Then run in another terminal:',
              "  $ks update",
              'Expect: the frozen programs run again, the console says "updated to version ...", one new tray icon appears (not two), its menu offers "Apply last profile". Then press Enter.')
        }) -Check {
        param($since)
        $null = Wait-Until 30 { Get-MainTray }
        $after = (Invoke-Ks --version).Output | Select-Object -First 1
        $trays = @(Get-TrayWindows | Where-Object { (Get-HelperPids) -notcontains [int]$_.Pid }).Count
        $f = Get-FreezeReport $script:Programs.suspend
        $oldNow = if (Test-Path -LiteralPath $oldExe) { Get-Sha256 $oldExe } else { $null }
        Set-State 'version' $after
        # Off again for the rest of the test.
        $null = Invoke-Ks update --mode off
        @{ ok = ($after -ne $before -and $trays -eq 1 -and $f.none -and $oldNow -and $oldNow -ne $oldHash)
           m = [ordered]@{ version_before = $before; version_after = $after; trays = $trays; frozen = $f.detail; old_exe_next_to_it = [bool]$oldNow } }
    } -Ask 'As described? Did SmartScreen warn when the new tray started (note it)?' -AskNote
}
function Send-SessionEnd($Tray) {
    if (-not $Tray) { throw 'no tray of the test runs; nothing to send the end of the session to' }
    $q = [KsBetaV3.Native]::Send($Tray.Handle, $script:WM_QUERYENDSESSION, 0, [int64]$script:ENDSESSION_LOGOFF, 10000)
    $e = [KsBetaV3.Native]::Send($Tray.Handle, $script:WM_ENDSESSION, 1, [int64]$script:ENDSESSION_LOGOFF, 10000)
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
    # Unpausing restarts the tray, which would resume what is frozen: first.
    if ($useTrigger) { $null = Request-Closed $tr; Set-TriggersPaused $false }
    # Until the session really ends (or collect takes over), automatic
    # switching is paused again on every way out: cancel, q, Ctrl+C, an
    # error, a refused shutdown.exe.
    $repause = $useTrigger
    try {
    $running = Request-Running ($script:Programs.suspend + $script:Programs.stop)
    Invoke-Step $id prepare "$(if ($id -eq 't7') { 'Sign-out' } else { 'Shutdown' }) with programs frozen: prepare ($Kind)" -Freeze -Prepare {
        $r = Switch-Profile 'beta-session'
        $m = [ordered]@{ switch_exit = $r.Code }
        if ($useTrigger) {
            $s = Get-UtcStamp
            $null = Start-Program $tr
            $null = Wait-Log 'switch to "beta-trigger" finished' $s 25
            $m.trigger_applied = (Get-Active) -eq 'beta-trigger'
        }
        $m
    } -Check {
        param($since)
        # What is frozen at sign-out is what the active profile freezes: with
        # the trigger, beta-trigger (only the first program).
        $active = Get-Active
        $script:Ctx.session_active = $active
        $script:Ctx.session_frozen = if ($active -eq 'beta-trigger') { @(Get-First) } else { $script:Programs.suspend }
        $f = Get-FreezeReport $script:Ctx.session_frozen
        @{ ok = $f.all; m = [ordered]@{ frozen = $f.detail; active = $active } }
    }
    # The tray offers the active profile, or for one a trigger applied the
    # profile from before the trigger (U4, decided detail 8).
    $active = $script:Ctx.session_active
    $expected = if ($active -eq 'beta-trigger') { 'beta-session' } else { $active }
    $boot = ConvertTo-Stamp (Get-CimInstance Win32_OperatingSystem).LastBootUpTime
    $marker = [pscustomobject]@{
        test = $id; since = (Get-UtcStamp (-2000)); expected_offer = $expected; active = $active; boot = $boot
        simulated = [bool]$SimulateSessionEnd; trigger = $useTrigger; sandbox = [bool]$script:State.sandbox
        frozen = @(Get-List $script:Ctx.session_frozen | ForEach-Object { $_.exe })
    }
    Set-State 'pending' $marker
    if ($SimulateSessionEnd) {
        Write-Note '   Simulated: the tray gets WM_QUERYENDSESSION and WM_ENDSESSION, then it is ended as Windows would.'
        $sim = Send-SessionEnd (Get-MainTray)
        Add-Result $id simulate 'pass' $true $null $sim 'session end simulated; run collect' (Get-UtcStamp)
        $repause = $false
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
        Set-Neutral
        Add-Result $id run 'skip' $null $null ([ordered]@{}) 'cancelled before the session ended' (Get-UtcStamp)
        return
    }
    $arg = if ($id -eq 't7') { @('/l') } elseif ($Restart) { @('/r', '/t', '0') } else { @('/s', '/t', '0') }
    # From here on the script does nothing more: no further tests, no resume.
    $script:SessionEnding = $true
    $repause = $false
    & shutdown.exe @arg
    if ($LASTEXITCODE -ne 0) {
        $script:SessionEnding = $false
        $repause = $useTrigger
        Set-State 'pending' $null
        Add-Result $id run 'fail' $false $null ([ordered]@{ shutdown_exit = $LASTEXITCODE }) 'shutdown.exe refused; nothing ended' (Get-UtcStamp)
        Invoke-WayOut
    }
    } finally {
        if ($repause) { try { $null = Close-Program $tr -Wait; Set-TriggersPaused $true } catch { Write-Bad "   could not pause automatic switching again: $($_.Exception.Message)" } }
    }
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
    # The report gets the log lines of the session end only, not the time between.
    $from = if ($start) { $start.Substring(0, 24) } else { $p.since }
    $to = if ($end) { $end.Substring(0, 24) } else { (ConvertFrom-UtcStamp $from).AddSeconds(10).ToString("yyyy-MM-dd'T'HH:mm:ss.fff'Z'", $script:Invariant) }
    Add-Result $id resume $(if ($start -and $end -and -not $timedOut) { 'pass' } else { 'fail' }) ([bool]($start -and $end -and -not $timedOut)) $null $m $(if ($ms) { "resume took $ms ms (the open 4 s wait, open-decisions deferred item 1)" }) $from $to

    $sessionEnd = if ($start) { $start.Substring(0, 24) } else { $p.since }
    $trayRan = [bool](Get-MainTray)
    $startedAfter = Find-Log 'tray: started (pid' $sessionEnd
    if ($startedAfter) {
        # The first lines after sign-in: the tray starting and its offer.
        $sf = $startedAfter.Substring(0, 24)
        Add-Result $id sign-in-log 'n/a' $null $null ([ordered]@{}) 'log lines of the tray start after the session end' $sf ((ConvertFrom-UtcStamp $sf).AddSeconds(15).ToString("yyyy-MM-dd'T'HH:mm:ss.fff'Z'", $script:Invariant))
    }
    if (-not $p.sandbox -and -not $p.simulated) {
        $rv = Get-RunValue $script:RealRunKey
        $ok = $trayRan -and [bool]$startedAfter -and $rv.present
        $sf = if ($startedAfter) { $startedAfter.Substring(0, 24) } else { Get-UtcStamp }
        Add-Result signin at-sign-in $(if ($ok) { 'pass' } else { 'fail' }) $ok $null ([ordered]@{ tray_running = $trayRan; started_line = $(if ($startedAfter) { $startedAfter.Substring(0, 23) }); run_value = $rv.data }) 'the tray started at sign-in' $sf $sf
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
    'programs'     = @{ title = 'Built-in test programs (self-check)'; ref = 'beta-test.ps1'; fn = 'Test-Programs' }
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
    if ($script:State.phase -eq 'restored') { throw "teardown has put your configuration back already: no more tests in this setup. Run 'beta-test.ps1 teardown -Uninstall' or 'teardown -KeepBeta' to finish (collect still works)." }
    if ($script:State.phase -ne 'ready') { throw "setup did not finish (phase '$($script:State.phase)'); run teardown, then setup again" }
    $Target = (@($Target) -join ',').Trim(',')
    if (-not $Target -or $Target -eq 'list') { Show-TestList; return }
    $ids = if ($Target -eq 'all') { @($script:Catalog.Keys | Where-Object { -not $script:Catalog[$_].session }) } else { @($Target.Split(',') | ForEach-Object { $_.Trim().ToLowerInvariant() }) }
    foreach ($id in $ids) { if (-not $script:Catalog.Contains($id)) { throw "unknown test '$id'; run 'beta-test.ps1 run list'" } }
    if (@($ids | Where-Object { $script:Catalog[$_].session }).Count -and $ids.Count -gt 1) { throw 't7 and t20 end the session: run each of them on its own' }
    Show-EnvWarning
    if ($script:State.pending) {
        if (@($ids | Where-Object { $script:Catalog[$_].session }).Count) { throw "a session test ($($script:State.pending.test)) waits for 'collect'; run collect first" }
        Write-Note "A session test ($($script:State.pending.test)) waits for 'collect'; run collect first after signing in again."
    }
    foreach ($id in $ids) {
        $t = $script:Catalog[$id]
        Write-Title "######## ${id}: $($t.title) ($($t.ref))"
        try { Invoke-Expression $t.fn }
        catch {
            if ($script:SessionEnding) { return }
            if (Test-Abort $_) { Write-Note 'Run stopped. Everything was resumed.'; return }
            Write-Bad "test $id failed: $($_.Exception.Message)"
            Add-Result $id error 'fail' $false $null ([ordered]@{}) ('error: ' + $_.Exception.Message) (Get-UtcStamp)
            try { Invoke-WayOut } catch {}
        }
        if ($script:SessionEnding) { return }
    }
    Show-TestList
}

# --- collect --------------------------------------------------------------------

# The report names neither the profile folder (long or 8.3 form, also with
# doubled backslashes as in JSON), the user, the domain, the computer nor an
# account's SID.
function Protect-Text([string]$Text) {
    if (-not $Text) { return $Text }
    $profiles = @(Get-LongPath $env:USERPROFILE; [KsBetaV3.Native]::ShortPath($env:USERPROFILE); $env:USERPROFILE) | Where-Object { $_ } | Sort-Object -Unique
    foreach ($pr in $profiles) {
        foreach ($form in @($pr, $pr.Replace('\', '\\'))) { $Text = [regex]::Replace($Text, [regex]::Escape($form), '%USERPROFILE%', 'IgnoreCase') }
    }
    # Security identifiers of accounts (the pipe name of the tray holds one).
    $Text = [regex]::Replace($Text, 'S-1-5-21(-\d+){3,4}', 'S-1-5-21-<sid>')
    foreach ($pair in @(@($env:USERNAME, '%USERNAME%'), @($env:COMPUTERNAME, '%COMPUTERNAME%'), @($env:USERDOMAIN, '%USERDOMAIN%'))) {
        if ($pair[0] -and $pair[0].Length -ge 2) {
            $Text = [regex]::Replace($Text, '(?<![A-Za-z0-9])' + [regex]::Escape($pair[0]) + '(?![A-Za-z0-9])', $pair[1], 'IgnoreCase')
        }
    }
    $Text
}

function Invoke-Collect {
    Use-State
    $restored = $script:State.phase -eq 'restored'
    if ($restored) {
        # Your own configuration is back: the report comes from the copies
        # teardown kept, and nothing is started or changed.
        $kept = Join-Path $script:Session 'at-teardown'
        if (-not (Test-Path -LiteralPath $kept)) { throw "teardown kept no copy of the test's files ($kept); collect before teardown next time" }
        $script:DataDir = $kept; $script:ConfigDir = $kept
        if ($script:State.pending) { Write-Note "  The pending $($script:State.pending.test) cannot be evaluated after teardown; it stays open in the report." }
    } else { Complete-PendingSession }
    $results = Get-Results
    if ($NoProgramLists) {
        # Output of "kaltstellen list" and "status --unassigned" names running programs.
        foreach ($r in $results) {
            if (-not $r.measure) { continue }
            foreach ($n in @($r.measure.PSObject.Properties | Where-Object { $_.Name -match '^(list |status --unassigned$|unassigned$)' } | ForEach-Object { $_.Name })) {
                $r.measure.PSObject.Properties.Remove($n)
            }
        }
    }
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
        $i = 0; $ts = ''
        # Assigned first: the function returns its lines as one array.
        $all = Get-LogLines '0000'
        foreach ($l in $all) {
            # A line without a time stamp belongs to the line before it.
            if ($l.Length -ge 24 -and $l[4] -eq '-' -and $l[10] -eq 'T') { $ts = $l.Substring(0, 24) }
            while ($i -lt $merged.Count -and [string]::CompareOrdinal($ts, $merged[$i].to) -gt 0) { $i++ }
            if ($i -lt $merged.Count -and [string]::CompareOrdinal($ts, $merged[$i].from) -ge 0) { $keep.Add($l) }
        }
        & $put 'kaltstellen-log-excerpt.txt' (($keep.ToArray()) -join "`r`n")
        $os = Get-CimInstance Win32_OperatingSystem
        $ubr = try { (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').UBR } catch { $null }
        $wv = try { (Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction Stop).pv } catch { $null }
        $ver = if ($restored) { $script:State.version } else { (Invoke-Ks --version).Output | Select-Object -First 1 }
        $info = [ordered]@{
            kaltstellen = $ver; installed_sha256 = $script:State.install.sha256; source = $script:State.install.source
            windows = $os.Caption; version = $os.Version; build = "$($os.BuildNumber).$ubr"; os_architecture = $os.OSArchitecture
            processor_architecture = $env:PROCESSOR_ARCHITECTURE; webview2_runtime = $wv
            powershell = $PSVersionTable.PSVersion.ToString(); sandbox = $script:State.sandbox; collected_utc = (Get-UtcStamp)
        }
        & $put 'system.json' ($info | ConvertTo-Json)
        # state.json names what was frozen or stopped (and their paths): left out with -NoProgramLists.
        $extra = @((Join-Path $script:ConfigDir 'settings.json'))
        if (-not $NoProgramLists) { $extra += Join-Path $script:DataDir 'state.json' }
        foreach ($f in $extra) {
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
        Write-Info "  $($keep.Count) kaltstellen.log line(s) in the excerpt"
        Write-Info '  It holds the results, the kaltstellen.log lines of the test steps, version, Windows version, state.json and settings.json.'
        if ($NoProgramLists) {
            Write-Info '  Left out (-NoProgramLists): the output of "kaltstellen list" and "status --unassigned", and state.json.'
            Write-Note '  Still in it: the results and log lines name the programs of beta-test.json and the test files (they are the test).'
        }
        else { Write-Note '  The results include the output of "kaltstellen list" and "status --unassigned": names of programs that ran. -NoProgramLists leaves them out.' }
        Write-Info '  Your profile folder, user name, domain and computer name are written as %USERPROFILE%, %USERNAME%, %USERDOMAIN%, %COMPUTERNAME%, account SIDs as S-1-5-21-<sid>. Look at it before you send it.'
    } finally { Remove-Tree $stage }
}

# --- teardown -------------------------------------------------------------------

function Restore-Folder($Backup) {
    $target = $Backup.path
    if ($Backup.existed) {
        # The backup must be complete and unchanged before the target is touched.
        $bad = if (Test-Path -LiteralPath $Backup.backup) { Test-Manifest $Backup.backup $Backup.files } elseif (@(Get-List $Backup.files).Count) { 'the backup folder is missing' } else { $null }
        if ($bad) { throw "backup $($Backup.backup) does not match its manifest ($bad); $target was left as it is" }
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

function Restore-RunValue($Saved, [string]$Subkey = $script:RealRunKey) {
    $rk = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($Subkey)
    try {
        if ($Saved.present) {
            $kind = [Microsoft.Win32.RegistryValueKind]$Saved.kind
            $rk.SetValue($script:RunValue, $Saved.data, $kind)
            "Run value kaltstellen restored: $($Saved.data)"
        } elseif ($null -ne $rk.GetValue($script:RunValue)) {
            $rk.DeleteValue($script:RunValue)
            'Run value kaltstellen removed (it did not exist before the test)'
        } else { 'Run value kaltstellen: absent, as before' }
    } finally { $rk.Close() }
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
    $preEntry = @(Get-List $s.install.preexisting | Where-Object { $_.name -eq 'kaltstellen.exe' }) | Select-Object -First 1
    # Your own executable is back already (an earlier -Uninstall): nothing to wait for.
    $ownBack = $preEntry -and $haveExe -and ((Get-Sha256 $script:KsExe) -eq $preEntry.sha256)
    $preExe = [bool]$preEntry -and -not $ownBack
    if ($Uninstall) {
        if ($preExe) { Write-Info "  put your earlier kaltstellen.exe back in $($s.install_dir) (sha256 checked; the beta goes)" }
        else { Write-Info "  uninstall $($script:KsExe)" }
    } elseif ($preExe -and -not $KeepBeta) {
        Write-Info "  $($script:KsExe) replaced your own kaltstellen.exe: it stays until 'teardown -Uninstall' (puts yours back) or 'teardown -KeepBeta'"
    }
    $restoredBefore = $s.phase -eq 'restored'
    if ($restoredBefore) { Write-Info '  (folders and Run value were restored by an earlier teardown)' }
    if ($DryRun) { Write-Info ''; Write-Info '[dry run] nothing was changed.'; return }

    $done = New-Object System.Collections.Generic.List[string]
    $problems = New-Object System.Collections.Generic.List[string]
    if (-not $restoredBefore) {
    # 1. kaltstellen: resume, neutral, quit.
    if ($haveExe) {
        foreach ($w in @(Get-OurProcesses | ForEach-Object { $_.Id } | ForEach-Object { Get-WindowsOf @($_) 'kaltstellen-main' })) {
            $null = [KsBetaV3.Native]::PostMessage($w.Handle, $script:WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
        }
        $r = Invoke-Ks recover
        $n = Invoke-Ks switch neutral
        $done.Add("recover: exit $($r.Code); neutral: exit $($n.Code)")
        Stop-HelperTrays
        # The main tray through "kaltstellen quit" where the build has it.
        $main = Get-MainTray
        if ($main) { Stop-Tray; if (-not (Get-Process -Id $main.Pid -ErrorAction SilentlyContinue)) { $done.Add("tray $($main.Pid) quit") } }
        foreach ($t in Get-TrayWindows) {
            if (Stop-TrayWindow $t 25) { $done.Add("tray $($t.Pid) quit") } else { $done.Add("tray $($t.Pid) ended") }
        }
        $r2 = Invoke-Ks recover
        if ($r2.Code -ne 0) { $problems.Add("recover after quit: exit $($r2.Code): $($r2.Text)") }
    } elseif ($s.phase -eq 'ready') {
        $done.Add("WARNING: $($script:KsExe) is missing, 'recover' could not run; if anything is still frozen, run recover with another kaltstellen.exe")
    }
    foreach ($p in Get-OurProcesses) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue; $done.Add("ended kaltstellen $($p.Id)") }
    Set-State 'helper_pids' @()
    # WebView2 processes of the main window that use this data folder (only
    # those: the command line names the folder) hold files in it.
    foreach ($id in Get-WebViewProcesses) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue; $done.Add("ended msedgewebview2 $id of the test's data folder") }
    Start-Sleep -Milliseconds 800
    # 2. Programs the test started.
    foreach ($e in Get-List $s.started) {
        $p = Get-RecordedProcess $e.pid $e.start_ticks $null
        if (-not $p) { continue }
        $inTemp = $p.Path -and (Test-Below (Get-LongPath $p.Path) $s.temp_root)
        if ($e.disposable -or $inTemp) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue; $done.Add("ended $($e.exe) ($($e.pid))") }
        else {
            $null = $p.CloseMainWindow()
            if (-not $p.WaitForExit(5000)) { $problems.Add("$($e.exe) ($($e.pid)) was started by the test and still runs; close it yourself") }
            else { $done.Add("closed $($e.exe) ($($e.pid))") }
        }
    }
    if ($s.temp_root) {
        foreach ($p in Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and (Test-Below (Get-LongPath $_.Path) $s.temp_root) }) {
            Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue; $done.Add("ended $($p.Name) ($($p.Id)) from the test folder")
        }
    }
    Start-Sleep -Milliseconds 800
    # 3. The real configuration, data and Run value: only while no kaltstellen
    # runs in this session (any copy could hold or write these files).
    $mayRestore = $true
    if (-not $s.sandbox -and $s.backup) {
        # Trays that were just asked to quit get a moment to end.
        $null = Wait-Until 10 { -not (Get-RunningKaltstellen).Count } 300
        $others = Get-RunningKaltstellen
        while ($others.Count -and -not $script:IsUnattended) {
            Write-Note "  kaltstellen still runs: $(($others | ForEach-Object { "$($_.Id) $($_.Path)" }) -join '; ')"
            $a = (Read-Host '  Quit it (tray menu Quit), then press Enter (s = do not restore now)').Trim().ToLowerInvariant()
            if ($a -eq 's') { break }
            $others = Get-RunningKaltstellen
        }
        if ($others.Count) {
            $problems.Add("kaltstellen still runs ($(($others | ForEach-Object { $_.Id }) -join ', ')): the configuration, data and Run value were not restored yet")
            $mayRestore = $false
        }
    }
    if (-not $s.sandbox -and $s.backup -and $mayRestore) {
        # The test's files as they were at the first teardown; a repeated
        # teardown must not replace them with what is there by then.
        $keep = Join-Path $s.session_dir 'at-teardown'
        if (-not (Test-Path -LiteralPath $keep)) {
            # Into a temporary folder first: at-teardown\ exists only complete.
            $partial = "$keep.partial"
            if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Recurse -Force }
            New-Item -ItemType Directory -Force -Path $partial | Out-Null
            foreach ($f in @('config.toml', 'settings.json') | ForEach-Object { Join-Path $s.config_dir $_ }) { if (Test-Path -LiteralPath $f) { Copy-Item -LiteralPath $f -Destination $partial -Force } }
            foreach ($f in @('kaltstellen.log', 'kaltstellen.log.1', 'kaltstellen.log.2', 'state.json', 'seen.json', 'update.json') | ForEach-Object { Join-Path $s.data_dir $_ }) { if (Test-Path -LiteralPath $f) { Copy-Item -LiteralPath $f -Destination $partial -Force } }
            Rename-Item -LiteralPath $partial -NewName 'at-teardown'
        }
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
    } else {
        # Restored already: what runs from the installed executable may be
        # your own tray by now. Only -Uninstall, which replaces the file,
        # ends it first.
        if ($Uninstall) {
            if ($haveExe) { Stop-Tray }
            foreach ($p in Get-OurProcesses) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue; $done.Add("ended kaltstellen $($p.Id) to replace its executable") }
        }
    }
    $install = $s.install
    $inTempInstall = Test-Below $s.install_dir $s.temp_root
    if ($Uninstall -and -not $inTempInstall -and $install) {
        foreach ($n in 'kaltstellen.exe', 'kaltstellen.old.exe') {
            $f = Join-Path $s.install_dir $n
            $pre = @(Get-List $install.preexisting | Where-Object { $_.name -eq $n })
            try {
                # An earlier copy goes back only after its check; without one
                # the beta is removed.
                if ($pre.Count) { $done.Add((Restore-SavedExe $pre[0] $s.session_dir $s.install_dir)) }
                elseif (Test-Path -LiteralPath $f) { Remove-Item -LiteralPath $f -Force; $done.Add("removed $f") }
            } catch { $problems.Add("$f`: $($_.Exception.Message) (or keep the beta: teardown -KeepBeta)") }
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
    if (-not $Uninstall -and -not $inTempInstall -and $preExe -and -not $KeepBeta) {
        # Everything else is back; the executable waits for the owner's choice.
        Set-State 'phase' 'restored'
        Write-Note "  $($script:KsExe) is still the beta: setup replaced your own kaltstellen.exe there (kept in $($s.session_dir)\backup\install)."
        Write-Note "  Run 'beta-test.ps1 teardown -Uninstall' to put yours back, or 'beta-test.ps1 teardown -KeepBeta' to keep the beta and finish."
        Show-QuitBefore
        return
    }
    if ($ownBack) { Write-Info "  your earlier kaltstellen.exe is in $($s.install_dir)" }
    elseif ($KeepBeta -and $preEntry -and -not $Uninstall) {
        Write-Info "  kept the beta in $($s.install_dir); your earlier kaltstellen.exe is saved as $($s.session_dir)\backup\install\kaltstellen.exe (sha256 $($preEntry.sha256))"
    }
    elseif (-not $Uninstall -and -not $inTempInstall) { Write-Info "  kept $($script:KsExe) (the beta; teardown -Uninstall would remove it)" }
    Move-Item -LiteralPath $script:StateFile -Destination (Join-Path $s.session_dir 'state.done.json') -Force
    Write-Info "  results and backups stay in $($s.session_dir)"
    Show-QuitBefore
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
        # Where, for a report of an unexpected error (a refusal of the script
        # itself, throw "...", has its message as the error id).
        if ($_.FullyQualifiedErrorId -ne $_.Exception.Message -and $_.InvocationInfo -and $_.InvocationInfo.ScriptLineNumber) { Write-Bad "  (beta-test.ps1 line $($_.InvocationInfo.ScriptLineNumber): $($_.InvocationInfo.Line.Trim()))" }
        $script:ExitCode = 1
    }
} finally {
    Restore-Environment
}
exit $script:ExitCode
