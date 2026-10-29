<#
.SYNOPSIS
  Builds the NabyScape release assets (x64 and ARM64) from the Rust workspace.

.DESCRIPTION
  Mirrors tools/Build-Release.ps1 of the C# releases and keeps its asset names, so the same
  GitHub release feed and updater work:
    NabyScape-win-x64-Setup.exe     NabyScape-win-arm64-Setup.exe
    NabyScape-win-x64-Portable.zip  NabyScape-win-arm64-Portable.zip
    NabyScape-Notices.zip           SHA256SUMS

  nabyscape.exe is its own installer. A Setup.exe is the program with a ZIP appended after the
  PE image holding the Whistle files (libneedle.dll, whistle.cact) and the notices; installing
  copies the exe byte for byte and unpacks that payload next to it. The Portable.zip holds the
  plain exe plus the same files.

  Whistle files come from rust/whistle.lock (written by the voice task): a JSON array of
  { "name", "arch" ("any"|"x64"|"arm64"), "url", "sha256" }. Every download is verified
  against its pinned SHA-256 before it is packaged. This script never writes the lock file.

.EXAMPLE
  ./build-release.ps1 -Tag v3.0.0
#>
param(
    [Parameter(Mandatory)][string]$Tag,
    [string]$OutputDirectory,
    [string]$LockFile = (Join-Path $PSScriptRoot 'whistle.lock'),
    [string]$Cargo = 'cargo',
    [switch]$SkipTests,
    [switch]$SkipSelfTest
)
$ErrorActionPreference = 'Stop'
if ($Tag -notmatch '^v\d+\.\d+\.\d+$') { throw 'Expected tag vMAJOR.MINOR.PATCH.' }
$workspace = $PSScriptRoot
$repo = Split-Path $workspace -Parent
if ([string]::IsNullOrEmpty($OutputDirectory)) { $OutputDirectory = Join-Path $repo 'release-output' }

$manifest = Get-Content -LiteralPath (Join-Path $workspace 'Cargo.toml') -Raw
if ($manifest -notmatch '(?ms)\[workspace\.package\].*?^version\s*=\s*"(\d+\.\d+\.\d+)"') { throw 'Could not read the workspace version.' }
if ($Tag.Substring(1) -ne $Matches[1]) { throw "Tag $Tag must match the workspace version $($Matches[1])." }

if (-not (Test-Path -LiteralPath $LockFile)) {
    throw "Missing $LockFile. The Whistle model and engine are pinned there by the voice task; the release cannot be built without it."
}
$lock = @(Get-Content -LiteralPath $LockFile -Raw | ConvertFrom-Json)
if ($lock.Count -eq 0) { throw "$LockFile lists no files." }
foreach ($entry in $lock) {
    if (-not $entry.name -or $entry.name -notmatch '^[A-Za-z0-9._-]+$') { throw "Invalid name in whistle.lock: $($entry.name)" }
    if ($entry.arch -notin 'any', 'x64', 'arm64') { throw "Invalid arch for $($entry.name) in whistle.lock." }
    if ($entry.sha256 -notmatch '^[A-Fa-f0-9]{64}$') { throw "Invalid sha256 for $($entry.name) in whistle.lock." }
    if ($entry.url -notmatch '^https://') { throw "Only https URLs are accepted ($($entry.name))." }
}

$output = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $output) { throw 'Release output must be a new empty path; never reuse prior assets.' }
New-Item -ItemType Directory -Path $output | Out-Null
$cache = Join-Path $output 'cache'
New-Item -ItemType Directory -Path $cache | Out-Null

function Get-Sha256([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }

function Get-Pinned($Entry) {
    $target = Join-Path $cache ($Entry.sha256.ToLowerInvariant() + '-' + $Entry.name)
    if (-not (Test-Path -LiteralPath $target)) {
        Write-Output "Downloading $($Entry.name) ($($Entry.arch))"
        Invoke-WebRequest -Uri $Entry.url -OutFile $target -UseBasicParsing
    }
    if ((Get-Sha256 $target) -ne $Entry.sha256.ToLowerInvariant()) {
        Remove-Item -LiteralPath $target -Force
        throw "SHA-256 mismatch for $($Entry.name); the download does not match rust/whistle.lock."
    }
    return $target
}

function Assert-PeMachine([string]$Path, [int]$Machine) {
    $stream = [IO.File]::OpenRead($Path); $reader = [IO.BinaryReader]::new($stream)
    try {
        if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5A4D) { throw "$Path is not a Windows executable." }
        $stream.Position = 0x3C; $offset = $reader.ReadInt32()
        if ($offset -lt 64 -or $offset -gt $stream.Length - 6) { throw "$Path has an invalid PE header." }
        $stream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne $Machine) { throw "$Path does not match the target architecture." }
    } finally { $reader.Dispose(); $stream.Dispose() }
}

# --- notices -----------------------------------------------------------------------------------
$notices = Join-Path $output 'notices'
New-Item -ItemType Directory -Path $notices | Out-Null
Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination (Join-Path $notices 'LICENSE')

Push-Location $workspace
try {
    $about = Get-Command cargo-about -ErrorAction SilentlyContinue
    if ($about -and (Test-Path -LiteralPath (Join-Path $workspace 'about.hbs')) -and (Test-Path -LiteralPath (Join-Path $workspace 'about.toml'))) {
        & $Cargo about generate about.hbs --workspace | Set-Content -LiteralPath (Join-Path $notices 'THIRD-PARTY-NOTICES.txt') -Encoding utf8
        if ($LASTEXITCODE -ne 0) { throw 'cargo about failed.' }
    } else {
        # Without cargo-about: every crate of the dependency graph with its declared license.
        $meta = & $Cargo metadata --format-version 1 --locked | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed.' }
        $workspaceIds = @($meta.workspace_members)
        $lines = @(
            'NabyScape third-party software notices',
            '',
            'NabyScape for Windows is built from the Rust crates below. Each is used under the license it declares;',
            'license texts are in the sources published with each crate.',
            ''
        )
        $lines += $meta.packages | Where-Object { $workspaceIds -notcontains $_.id } | Sort-Object name, version | ForEach-Object {
            $license = if ($_.license) { $_.license } else { 'see crate sources' }
            "$($_.name) $($_.version) - $license - $($_.repository)"
        }
        $lines | Set-Content -LiteralPath (Join-Path $notices 'THIRD-PARTY-NOTICES.txt') -Encoding utf8
    }
} finally { Pop-Location }

@'
Cactus Whistle speech recognition

NabyScape ships the Cactus Compute Whistle speech-to-text model (whistle.cact, Hugging Face
Cactus-Compute/whistle) and the Cactus Needle engine library (libneedle.dll, Hugging Face
Cactus-Compute/needle3, engine 3.3.3) unmodified. The Needle engine is licensed under Apache-2.0
(https://www.apache.org/licenses/LICENSE-2.0). Both are loaded only while voice input is active and
run entirely on this PC; no audio leaves the device for the default listening model.

Whistle recognizes English, German, French, Spanish, Italian, Dutch and Polish (auto-detected).
The exact files and SHA-256 hashes shipped are pinned in rust/whistle.lock of the source tree.
'@ | Set-Content -LiteralPath (Join-Path $notices 'WHISTLE-NOTICE.txt') -Encoding utf8
$noticeFiles = @('LICENSE', 'THIRD-PARTY-NOTICES.txt', 'WHISTLE-NOTICE.txt')

# --- build per architecture ----------------------------------------------------------------------
$targets = @(
    @{ Rid = 'win-x64';   Arch = 'x64';   Triple = 'x86_64-pc-windows-msvc';  Machine = 0x8664 },
    @{ Rid = 'win-arm64'; Arch = 'arm64'; Triple = 'aarch64-pc-windows-msvc'; Machine = 0xAA64 }
)
$hostArch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
$assets = @()
Push-Location $workspace
try {
    if (-not $SkipTests) {
        & $Cargo test --workspace --locked
        if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
    }
    foreach ($t in $targets) {
        & rustup target add $t.Triple | Out-Null
        & $Cargo build --release --locked -p naby-win --target $t.Triple
        if ($LASTEXITCODE -ne 0) { throw "Build failed for $($t.Rid)." }
        $exe = Join-Path $workspace "target/$($t.Triple)/release/nabyscape.exe"
        Assert-PeMachine $exe $t.Machine

        $stage = Join-Path $output "stage/$($t.Rid)"
        New-Item -ItemType Directory -Path $stage | Out-Null
        Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'NabyScape.exe')
        $payload = @()
        foreach ($entry in $lock | Where-Object { $_.arch -in 'any', $t.Arch }) {
            $file = Get-Pinned $entry
            Copy-Item -LiteralPath $file -Destination (Join-Path $stage $entry.name)
            $payload += $entry.name
        }
        foreach ($required in 'libneedle.dll', 'whistle.cact') {
            if ($payload -notcontains $required) { throw "whistle.lock has no $required for $($t.Arch)." }
        }
        if (Test-Path -LiteralPath (Join-Path $stage 'libneedle.dll')) { Assert-PeMachine (Join-Path $stage 'libneedle.dll') $t.Machine }
        foreach ($n in $noticeFiles) { Copy-Item -LiteralPath (Join-Path $notices $n) -Destination (Join-Path $stage $n) }

        # Self-test where this host can run the binary: payload present, store, fonts, version.
        if (-not $SkipSelfTest -and $t.Arch -eq $hostArch) {
            $report = Join-Path $output "self-test-$($t.Rid).txt"
            $p = Start-Process -FilePath (Join-Path $stage 'NabyScape.exe') -ArgumentList @('--self-test', '--require-payload', '--report', ('"' + $report + '"')) -WindowStyle Hidden -PassThru -Wait
            if ($p.ExitCode -ne 0) { throw "Self-test failed for $($t.Rid); see $report." }
        } else { Write-Output "Self-test not executed for $($t.Rid) (host $hostArch, SkipSelfTest=$SkipSelfTest)." }

        # Portable ZIP: the plain program plus its files.
        $portable = Join-Path $output "NabyScape-$($t.Rid)-Portable.zip"
        $files = @('NabyScape.exe') + $payload + $noticeFiles | ForEach-Object { Join-Path $stage $_ }
        Compress-Archive -LiteralPath $files -DestinationPath $portable

        # Setup: the program with the payload ZIP appended after the PE image.
        $payloadZip = Join-Path $output "stage/$($t.Rid)-payload.zip"
        Compress-Archive -LiteralPath (@($payload + $noticeFiles | ForEach-Object { Join-Path $stage $_ })) -DestinationPath $payloadZip
        $setup = Join-Path $output "NabyScape-$($t.Rid)-Setup.exe"
        Copy-Item -LiteralPath (Join-Path $stage 'NabyScape.exe') -Destination $setup
        $out = [IO.File]::Open($setup, 'Append', 'Write')
        try { $in = [IO.File]::OpenRead($payloadZip); try { $in.CopyTo($out) } finally { $in.Dispose() } } finally { $out.Dispose() }
        Assert-PeMachine $setup $t.Machine
        $assets += @($setup, $portable)
    }
} finally { Pop-Location }

$noticeZip = Join-Path $output 'NabyScape-Notices.zip'
Compress-Archive -LiteralPath @($noticeFiles | ForEach-Object { Join-Path $notices $_ }) -DestinationPath $noticeZip
$assets += $noticeZip
$assets | ForEach-Object { (Get-Sha256 $_) + '  ' + [IO.Path]::GetFileName($_) } | Set-Content -LiteralPath (Join-Path $output 'SHA256SUMS') -Encoding ascii

# Keep only the release assets in the output folder.
Remove-Item -LiteralPath (Join-Path $output 'stage'), $cache, $notices -Recurse -Force

$expected = @('NabyScape-win-x64-Setup.exe', 'NabyScape-win-arm64-Setup.exe', 'NabyScape-win-x64-Portable.zip', 'NabyScape-win-arm64-Portable.zip', 'NabyScape-Notices.zip', 'SHA256SUMS')
foreach ($name in $expected) { if (-not (Test-Path -LiteralPath (Join-Path $output $name))) { throw "Missing release asset $name." } }
Write-Output "Release assets written to $output (cross-architecture binaries are never executed)."
