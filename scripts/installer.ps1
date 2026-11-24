<#
.SYNOPSIS
Builds the per-user setup of kaltstellen with Inno Setup (spec Q2).

.DESCRIPTION
Compiles installer/kaltstellen.iss around one executable. Without -Iscc it
uses Inno Setup from -ToolsDir, and fetches the pinned version there first
if it is missing: the official installer from the Inno Setup releases on
GitHub, checked by SHA-256 and Authenticode signature, installed portable
(no registry entries, no Start menu).

.PARAMETER Exe
The executable to install (target\release\kaltstellen.exe or a release file).

.PARAMETER Arch
x86_64 or aarch64 (std::env::consts::ARCH of the executable). Default: the
architecture of this system.

.PARAMETER Version
What the setup calls the version. Default: what `<Exe> --version` reports.

.PARAMETER OutputDir
Where the setup goes. Default: next to -Exe.

.PARAMETER OutputName
File name of the setup without .exe. Default:
kaltstellen-<version>-windows-<arch>-setup.

.PARAMETER Iscc
ISCC.exe to use instead of the pinned Inno Setup.

.PARAMETER ToolsDir
Where the pinned Inno Setup is kept. Default: %TEMP%\kaltstellen-tools.

.OUTPUTS
The full path of the setup.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Exe,
    [ValidateSet('x86_64', 'aarch64')][string]$Arch,
    [string]$Version,
    [string]$OutputDir,
    [string]$OutputName,
    [string]$Iscc,
    [string]$ToolsDir = (Join-Path ([IO.Path]::GetTempPath()) 'kaltstellen-tools')
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3

# Pinned: the same file and hash as the winget manifest of this version.
$InnoVersion = '6.7.3'
$InnoUrl = 'https://github.com/jrsoftware/issrc/releases/download/is-6_7_3/innosetup-6.7.3.exe'
$InnoSha256 = '9c73c3bae7ed48d44112a0f48e66742c00090bdb5bef71d9d3c056c66e97b732'
$InnoSigner = 'Pyrsys B.V.'

function Get-PinnedIscc([string]$Dir) {
    $home_ = Join-Path $Dir "innosetup-$InnoVersion"
    $iscc = Join-Path $home_ 'ISCC.exe'
    if (Test-Path -LiteralPath $iscc) { return $iscc }
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $setup = Join-Path $Dir "innosetup-$InnoVersion.exe"
    if (-not (Test-Path -LiteralPath $setup)) {
        Write-Host "downloading Inno Setup $InnoVersion"
        # Windows PowerShell 5.1 does not offer TLS 1.2 by itself.
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        $ProgressPreference = 'SilentlyContinue'
        Invoke-WebRequest -Uri $InnoUrl -OutFile "$setup.part" -UseBasicParsing
        Move-Item -LiteralPath "$setup.part" -Destination $setup -Force
    }
    $hash = (Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -ne $InnoSha256) {
        Remove-Item -LiteralPath $setup -Force
        throw "Inno Setup download has SHA-256 $hash, expected $InnoSha256"
    }
    $sig = Get-AuthenticodeSignature -LiteralPath $setup
    if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch [regex]::Escape("CN=$InnoSigner")) {
        throw "Inno Setup download is not validly signed by $InnoSigner ($($sig.Status))"
    }
    # Portable: no uninstall entry, no Start menu, nothing in the registry.
    $p = Start-Process -FilePath $setup -Wait -PassThru -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER',
        '/PORTABLE=1', '/NOICONS', "/DIR=$home_")
    if ($p.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $iscc)) {
        throw "installing Inno Setup $InnoVersion failed (exit code $($p.ExitCode))"
    }
    return $iscc
}

$Exe = (Resolve-Path -LiteralPath $Exe).Path
if (-not $Arch) {
    $Arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
}
if (-not $Version) {
    $line = (& $Exe --version | Select-Object -First 1)
    if ($LASTEXITCODE -ne 0 -or $line -notmatch '^kaltstellen (\S+)$') {
        throw "cannot read the version of $Exe ($line)"
    }
    $Version = $Matches[1]
}
# The file version has four numbers: the release numbers, then 0.
if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)') { throw "version $Version does not start with x.y.z" }
$numeric = "$($Matches[1]).$($Matches[2]).$($Matches[3]).0"
if (-not $OutputDir) { $OutputDir = Split-Path -Parent $Exe }
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path
# `+` (a development build) is not welcome in file names.
if (-not $OutputName) { $OutputName = "kaltstellen-$($Version -replace '\+', '_')-windows-$Arch-setup" }
if (-not $Iscc) { $Iscc = Get-PinnedIscc $ToolsDir }

$script = Join-Path (Split-Path -Parent $PSScriptRoot) 'installer\kaltstellen.iss'
# To the console, not into the output: the caller takes the path from it.
& $Iscc /Qp "/DVersion=$Version" "/DNumericVersion=$numeric" "/DArch=$Arch" "/DExe=$Exe" `
    "/DOutputDir=$OutputDir" "/DOutputName=$OutputName" $script | Out-Host
if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }
$out = Join-Path $OutputDir "$OutputName.exe"
if (-not (Test-Path -LiteralPath $out)) { throw "ISCC did not write $out" }
$out
