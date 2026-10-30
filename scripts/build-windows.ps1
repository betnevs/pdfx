#Requires -Version 5.1
<#
.SYNOPSIS
    Builds the unsigned Windows x64 installers and portable zip for PDFX.

.DESCRIPTION
    Produces three artifacts for Windows 10 1809+/11 x64:

      * PDFX_<version>_x64_en-US.msi        (WiX MSI, WebView2 offline installer)
      * PDFX_<version>_x64-setup.exe        (NSIS, WebView2 offline installer)
      * PDFX_<version>_x64-portable.zip     (PDFX.exe + pdfium.dll + licenses)

    The script fetches the pinned PDFium DLL with `npm run setup:native`,
    builds with the `x86_64-pc-windows-msvc` target, assembles the portable
    zip, and writes SHA256SUMS-windows.txt. It does not sign anything.

    Prerequisites: Windows 10 1809+/11 x64, Node.js (^22.13.0 or >=24), Rust
    with the x86_64-pc-windows-msvc target, the MSVC build tools, and Git for
    Windows (its bash runs scripts/setup-native.sh).

.PARAMETER SkipNativeSetup
    Reuse an already-downloaded src-tauri/vendor/pdfium/ tree instead of
    running `npm run setup:native`. The DLL is still verified as PE32+ x64.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -SkipNativeSetup
#>
[CmdletBinding()]
param(
    [switch]$SkipNativeSetup
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ScriptDir = $PSScriptRoot
$RepoRoot = Split-Path -Parent $ScriptDir
Set-Location -Path $RepoRoot

$TargetTriple = 'x86_64-pc-windows-msvc'
$BundleTypes = 'msi,nsis'

function Write-Step {
    param([Parameter(Mandatory = $true)][string]$Message)
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Fail {
    param([Parameter(Mandatory = $true)][string]$Message)
    Write-Host ""
    Write-Host "ERROR: $Message" -ForegroundColor Red
    exit 1
}

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )
    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        Fail "$FilePath $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
}

function Assert-Pe64 {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [switch]$Dll
    )
    $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read)
    try {
        $reader = [System.IO.BinaryReader]::new($stream)
        if ($reader.ReadUInt16() -ne 0x5A4D) {
            Fail "$Path is not a PE file (missing MZ signature)."
        }
        [void]$stream.Seek(0x3C, [System.IO.SeekOrigin]::Begin)
        $peOffset = $reader.ReadInt32()
        if ($peOffset -le 0 -or $peOffset -ge $stream.Length) {
            Fail "$Path has an invalid PE header offset."
        }
        [void]$stream.Seek($peOffset, [System.IO.SeekOrigin]::Begin)
        if ($reader.ReadUInt32() -ne 0x00004550) {
            Fail "$Path is not a PE file (missing PE signature)."
        }
        $machine = $reader.ReadUInt16()
        if ($machine -ne 0x8664) {
            Fail ("$Path is not x64 (machine 0x{0:X4})." -f $machine)
        }
        # Machine is already consumed. Skip NumberOfSections, TimeDateStamp,
        # PointerToSymbolTable and NumberOfSymbols, then read
        # SizeOfOptionalHeader and Characteristics from the COFF header.
        [void]$stream.Seek(14, [System.IO.SeekOrigin]::Current)
        [void]$reader.ReadUInt16()
        $characteristics = $reader.ReadUInt16()
        if ($Dll -and (($characteristics -band 0x2000) -eq 0)) {
            Fail "$Path is a PE image but not a DLL."
        }
    }
    finally {
        $stream.Dispose()
    }
}

# --- host checks -----------------------------------------------------------
if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    Fail "the Windows installers must be built on Windows (found $([System.Environment]::OSVersion.Platform))."
}

$ProcessorArchitecture = [string]$env:PROCESSOR_ARCHITECTURE
if ($ProcessorArchitecture -ne 'AMD64') {
    Fail "the PDFium binary and the installers target x64; build on x64 Windows (found $ProcessorArchitecture)."
}

foreach ($tool in @('node', 'npm', 'npx', 'cargo', 'rustup')) {
    if (-not (Get-Command -Name $tool -ErrorAction SilentlyContinue)) {
        Fail "required tool not found on PATH: $tool"
    }
}

# scripts/setup-native.sh must run under Git Bash (not WSL bash, which would
# download the Linux PDFium .so). Prefer Git for Windows when it is installed.
$GitBashCandidates = @(
    (Join-Path ${env:ProgramFiles} 'Git\bin\bash.exe'),
    (Join-Path ${env:LOCALAPPDATA} 'Programs\Git\bin\bash.exe')
)
$GitBash = $GitBashCandidates | Where-Object { Test-Path -Path $_ -PathType Leaf } | Select-Object -First 1
if ($null -ne $GitBash) {
    $env:PATH = "$(Split-Path -Parent $GitBash);$env:PATH"
}
elseif (-not (Get-Command -Name 'bash' -ErrorAction SilentlyContinue)) {
    Fail "bash was not found. Install Git for Windows (it provides the bash used by scripts/setup-native.sh) or add its bin directory to PATH."
}

$NodeVersion = (& node --version).Trim()
$NodeMatch = [regex]::Match($NodeVersion, '^v(\d+)\.(\d+)\.')
if (-not $NodeMatch.Success) {
    Fail "could not parse the Node.js version: $NodeVersion"
}
$NodeMajor = [int]$NodeMatch.Groups[1].Value
$NodeMinor = [int]$NodeMatch.Groups[2].Value
if (-not (($NodeMajor -eq 22 -and $NodeMinor -ge 13) -or $NodeMajor -ge 24)) {
    Fail "Node.js $NodeVersion is not supported. Install Node.js ^22.13.0 or >=24."
}

$InstalledTargets = @(& rustup target list --installed | ForEach-Object { $_.Trim() })
if ($InstalledTargets -notcontains $TargetTriple) {
    Fail "the Rust target $TargetTriple is not installed.`n       Run: rustup target add $TargetTriple"
}

# --- JavaScript dependencies ----------------------------------------------
if (-not (Test-Path -Path (Join-Path $RepoRoot 'node_modules') -PathType Container)) {
    Write-Step 'node_modules is missing; running npm ci'
    Invoke-Native -FilePath 'npm' -Arguments @('ci')
}
if (-not (Test-Path -Path (Join-Path $RepoRoot 'node_modules\.bin\tauri.cmd') -PathType Leaf)) {
    Fail "the Tauri CLI is not installed. Run: npm ci"
}

# --- native PDFium ---------------------------------------------------------
if (-not $SkipNativeSetup) {
    Write-Step 'Fetching and verifying PDFium 151.0.7881.0'
    Invoke-Native -FilePath 'npm' -Arguments @('run', 'setup:native')
}

$PdfiumDll = Join-Path $RepoRoot 'src-tauri\vendor\pdfium\bin\pdfium.dll'
if (-not (Test-Path -Path $PdfiumDll -PathType Leaf)) {
    Fail "$PdfiumDll is missing after npm run setup:native. Re-run: npm run setup:native -- --force"
}
Assert-Pe64 -Path $PdfiumDll -Dll
$PdfiumLicense = Join-Path $RepoRoot 'src-tauri\vendor\pdfium\LICENSE'
$PdfiumLicenses = Join-Path $RepoRoot 'src-tauri\vendor\pdfium\licenses'
if (-not (Test-Path -Path $PdfiumLicense -PathType Leaf) -or
    -not (Test-Path -Path $PdfiumLicenses -PathType Container)) {
    Fail 'PDFium license files are missing from src-tauri/vendor/pdfium/. Re-run: npm run setup:native -- --force'
}
Write-Step "PDFium is a PE32+ x64 DLL with license files: $PdfiumDll"

# --- product metadata ------------------------------------------------------
$ConfigPath = Join-Path $RepoRoot 'src-tauri\tauri.conf.json'
$Config = Get-Content -Path $ConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
$CargoToml = Get-Content -Path (Join-Path $RepoRoot 'src-tauri\Cargo.toml') -Raw -Encoding UTF8
$PackageJson = Get-Content -Path (Join-Path $RepoRoot 'package.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$CargoName = [regex]::Match($CargoToml, '(?m)^name\s*=\s*"([^"]+)"').Groups[1].Value
$CargoVersion = [regex]::Match($CargoToml, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if ([string]::IsNullOrWhiteSpace($CargoName) -or [string]::IsNullOrWhiteSpace($CargoVersion)) {
    Fail 'could not read the package name or version from src-tauri/Cargo.toml.'
}
$ProductName = [string]$Config.productName
$Version = [string]$Config.version
if ([string]::IsNullOrWhiteSpace($ProductName) -or [string]::IsNullOrWhiteSpace($Version)) {
    Fail 'productName or version is missing from src-tauri/tauri.conf.json.'
}
if ($Version -ne $CargoVersion -or $Version -ne [string]$PackageJson.version) {
    Fail "version mismatch: tauri.conf.json=$Version Cargo.toml=$CargoVersion package.json=$($PackageJson.version)"
}
Write-Step "Building $ProductName $Version for $TargetTriple (MSI + NSIS, unsigned)"
Write-Host 'The first bundle needs network access to download the WebView2 Evergreen Standalone Installer;'
Write-Host 'the resulting installers themselves install WebView2 offline.'

# --- build -----------------------------------------------------------------
# tauri.windows.conf.json is merged automatically on Windows; it maps
# vendor/pdfium/bin/pdfium.dll into the resource directory as pdfium.dll.
Invoke-Native -FilePath 'npx' -Arguments @(
    'tauri', 'build',
    '--target', $TargetTriple,
    '--bundles', $BundleTypes,
    '--ci',
    '--no-sign'
)

# --- locate artifacts ------------------------------------------------------
$ReleaseDir = Join-Path $RepoRoot "src-tauri\target\$TargetTriple\release"
$BundleDir = Join-Path $ReleaseDir 'bundle'
$MsiDir = Join-Path $BundleDir 'msi'
$NsisDir = Join-Path $BundleDir 'nsis'
$PortableDir = Join-Path $BundleDir 'portable'

function Find-SingleArtifact {
    param(
        [Parameter(Mandatory = $true)][string]$Directory,
        [Parameter(Mandatory = $true)][string]$Filter,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if (-not (Test-Path -Path $Directory -PathType Container)) {
        Fail "$Label bundle directory is missing: $Directory"
    }
    $items = @(Get-ChildItem -Path $Directory -Filter $Filter -File)
    if ($items.Count -eq 0) {
        Fail "$Label artifact not found in $Directory (filter: $Filter)."
    }
    if ($items.Count -gt 1) {
        $names = ($items | ForEach-Object { $_.Name }) -join ', '
        Fail "expected one $Label artifact, found $($items.Count): $names"
    }
    return $items[0]
}

$Msi = Find-SingleArtifact -Directory $MsiDir -Filter '*.msi' -Label 'MSI'
$Nsis = Find-SingleArtifact -Directory $NsisDir -Filter '*-setup.exe' -Label 'NSIS installer'
$ExpectedPrefix = "${ProductName}_${Version}_x64"
if (-not $Msi.Name.StartsWith($ExpectedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    Fail "MSI name '$($Msi.Name)' does not start with '$ExpectedPrefix'."
}
if (-not $Nsis.Name.StartsWith("$ExpectedPrefix-setup", [System.StringComparison]::OrdinalIgnoreCase)) {
    Fail "NSIS installer name '$($Nsis.Name)' does not start with '$ExpectedPrefix-setup'."
}

$ExePath = Join-Path $ReleaseDir "$CargoName.exe"
if (-not (Test-Path -Path $ExePath -PathType Leaf)) {
    Fail "release executable is missing: $ExePath"
}
Assert-Pe64 -Path $ExePath

# --- portable zip ----------------------------------------------------------
New-Item -ItemType Directory -Path $PortableDir -Force | Out-Null
$PortableName = "${ExpectedPrefix}-portable"
$PortableStage = Join-Path $PortableDir $PortableName
$PortableZip = Join-Path $PortableDir "$PortableName.zip"
$LicenseStage = Join-Path $PortableStage 'pdfium-licenses'
$FontLicenseStage = Join-Path $PortableStage 'font-licenses'

if (Test-Path -Path $PortableStage) {
    Remove-Item -Path $PortableStage -Recurse -Force
}
New-Item -ItemType Directory -Path $LicenseStage -Force | Out-Null
New-Item -ItemType Directory -Path $FontLicenseStage -Force | Out-Null

Copy-Item -Path $ExePath -Destination (Join-Path $PortableStage "$ProductName.exe") -Force
Copy-Item -Path $PdfiumDll -Destination (Join-Path $PortableStage 'pdfium.dll') -Force
Copy-Item -Path $PdfiumLicense -Destination (Join-Path $LicenseStage 'LICENSE') -Force
Copy-Item -Path (Join-Path $PdfiumLicenses '*') -Destination $LicenseStage -Recurse -Force
Copy-Item -Path (Join-Path $RepoRoot 'src-tauri\fonts\*-OFL.txt') -Destination $FontLicenseStage -Force
Copy-Item -Path (Join-Path $RepoRoot 'src-tauri\fonts\SOURCE.txt') -Destination $FontLicenseStage -Force
$InstallNotes = Join-Path $RepoRoot 'docs\install-windows.md'
if (-not (Test-Path -Path $InstallNotes -PathType Leaf)) {
    Fail "install notes are missing: $InstallNotes"
}
Copy-Item -Path $InstallNotes -Destination (Join-Path $PortableStage 'INSTALL-windows.md') -Force

if (Test-Path -Path $PortableZip -PathType Leaf) {
    Remove-Item -Path $PortableZip -Force
}
Compress-Archive -Path (Join-Path $PortableStage '*') -DestinationPath $PortableZip -CompressionLevel Optimal

Add-Type -AssemblyName System.IO.Compression.FileSystem
$Archive = [System.IO.Compression.ZipFile]::OpenRead($PortableZip)
try {
    $Entries = @($Archive.Entries | ForEach-Object { $_.FullName.Replace('\', '/') })
    foreach ($RequiredEntry in @("$ProductName.exe", 'pdfium.dll', 'INSTALL-windows.md', 'pdfium-licenses/LICENSE', 'font-licenses/SOURCE.txt', 'font-licenses/NotoSansSC-OFL.txt')) {
        if ($Entries -notcontains $RequiredEntry) {
            Fail "portable zip is missing $RequiredEntry."
        }
    }
}
finally {
    $Archive.Dispose()
}
Remove-Item -Path $PortableStage -Recurse -Force

# --- checksums -------------------------------------------------------------
$Artifacts = @($Msi, $Nsis, (Get-Item -Path $PortableZip))
$ManifestPath = Join-Path $BundleDir 'SHA256SUMS-windows.txt'
$ChecksumLines = @()
foreach ($Artifact in $Artifacts) {
    $Hash = (Get-FileHash -Path $Artifact.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    $ChecksumLines += "$Hash  $($Artifact.Name)"
}
Set-Content -Path $ManifestPath -Value $ChecksumLines -Encoding ASCII

# --- summary ---------------------------------------------------------------
Write-Step 'Build complete (unsigned; SmartScreen bypass required on first launch)'
foreach ($Artifact in $Artifacts) {
    $SizeMb = [math]::Round($Artifact.Length / 1MB, 1)
    Write-Host ("  {0}  ({1} MB)" -f $Artifact.FullName, $SizeMb)
}
Write-Host "  $ManifestPath"
Write-Host ""
Write-Host 'Install instructions: docs\install-windows.md'
Write-Host 'Not verified by this script: installation/launch on Windows 10 1809+/11 x64,'
Write-Host 'offline WebView2 installation, and SmartScreen behavior. Test on a clean Windows VM.'
