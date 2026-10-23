# The Windows twin of prepare-desktop.sh: puts the desktop app's sidecars where `tauri build`
# takes them from (`bundle.externalBin` in apps/desktop/src-tauri/tauri.bundle.conf.json):
#
#   sidecars\bin\filebridge-service-<target triple>.exe   built here, in release
#   sidecars\bin\filebridge-<target triple>.exe           the CLI (apps/cli), built here
#   sidecars\bin\filebridge-rclone-<target triple>.exe    the pinned rclone (sidecars\RCLONE_VERSION)
#
# `pnpm bundle` in apps\desktop runs it first (beforeBuildCommand, through
# apps\desktop\scripts\prepare-sidecars.mjs). Windows PowerShell 5.1 or PowerShell 7. rclone comes
# from, in this order:
#
#   $env:RCLONE_DOWNLOAD = "1"   the pinned release from https://downloads.rclone.org/<version>/,
#                                for the target, checked against that release's SHA256SUMS before
#                                anything runs it (and the PGP signature on SHA256SUMS when gpg,
#                                e.g. Gpg4win, has rclone's key; with RCLONE_REQUIRE_SIGNATURE=1
#                                it must). Kept in sidecars\bin\download\.
#   $env:RCLONE = "C:\path"      that file.
#   otherwise                    rclone.exe on PATH.
#
# A local rclone is used only when `rclone version` is the pinned one. The man pages and shell
# completions the Linux packages carry are not made here.

#Requires -Version 5.1
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3

$here = $PSScriptRoot
$root = Split-Path $here -Parent
$version = (Get-Content -Raw (Join-Path $here 'RCLONE_VERSION')).Trim()
# rclone's release signing key (https://rclone.org/install/).
$rcloneKey = 'FBF737ECE9F8AB18604BD2AC93935E02FF3B54FA'

function Invoke-Native {
    param([string] $Exe, [string[]] $Arguments)
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Exe $($Arguments -join ' ') failed (exit code $LASTEXITCODE)" }
}

if ($env:CARGO) {
    $cargo = $env:CARGO
} elseif (Get-Command cargo -ErrorAction SilentlyContinue) {
    $cargo = (Get-Command cargo).Source
} else {
    $cargo = Join-Path $HOME '.cargo\bin\cargo.exe'
}
$rustc = Join-Path (Split-Path $cargo) 'rustc.exe'
if (-not (Test-Path $rustc)) { $rustc = (Get-Command rustc).Source }
$hostTriple = ((& $rustc -vV) | Where-Object { $_ -like 'host: *' }) -replace '^host: ', ''
$triple = if ($env:TAURI_ENV_TARGET_TRIPLE) { $env:TAURI_ENV_TARGET_TRIPLE } else { $hostTriple }
$ext = if ($triple -like '*windows*') { '.exe' } else { '' }

# rclone's name for the target's platform, as in its release file names.
function Get-RclonePlatform([string] $t) {
    switch -Wildcard ($t) {
        'x86_64-pc-windows-*' { return 'windows-amd64' }
        'aarch64-pc-windows-*' { return 'windows-arm64' }
        'i686-pc-windows-*' { return 'windows-386' }
        'x86_64-*-linux-*' { return 'linux-amd64' }
        'aarch64-*-linux-*' { return 'linux-arm64' }
        'x86_64-apple-darwin' { return 'osx-amd64' }
        'aarch64-apple-darwin' { return 'osx-arm64' }
    }
    return $null
}

# --- filebridge-service and the CLI -------------------------------------------------------------

# `tauri build --debug` bundles a debug build; the service matches it.
$buildProfile = if ($env:TAURI_ENV_DEBUG -eq 'true') { 'debug' } else { 'release' }
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$cargoArgs = @('build', '-p', 'filebridge-service', '-p', 'filebridge-cli', '--manifest-path', (Join-Path $root 'Cargo.toml'))
if ($buildProfile -eq 'release') { $cargoArgs += '--release' }
$built = Join-Path $targetDir $buildProfile
if ($triple -ne $hostTriple) {
    $cargoArgs += @('--target', $triple)
    $built = Join-Path (Join-Path $targetDir $triple) $buildProfile
}
Invoke-Native $cargo $cargoArgs

$out = Join-Path $here 'bin'
New-Item -ItemType Directory -Force -Path $out | Out-Null
foreach ($name in @('filebridge-service', 'filebridge')) {
    $dest = Join-Path $out "$name-$triple$ext"
    Copy-Item -Force (Join-Path $built "$name$ext") $dest
    Write-Host "sidecar: $dest"
}

# --- rclone -------------------------------------------------------------------------------------

# The SHA-256 SHA256SUMS gives for $file, from its signed part only.
function Get-ExpectedSha256([string] $sums, [string] $file) {
    $signed = $false
    foreach ($line in Get-Content $sums) {
        if ($line -like '-----BEGIN PGP SIGNATURE-----*') { break }
        if ($line -eq '') { $signed = $true; continue }
        if ($signed) {
            $fields = $line -split '\s+'
            if ($fields.Count -ge 2 -and $fields[1] -eq $file) { return $fields[0] }
        }
    }
    return $null
}

# The PGP signature on SHA256SUMS, by rclone's key. Throws on a bad signature; without gpg or the
# key it says so and goes on, unless RCLONE_REQUIRE_SIGNATURE=1.
function Test-Signature([string] $sums) {
    $gpg = Get-Command gpg -ErrorAction SilentlyContinue
    if ($gpg) {
        # Windows PowerShell turns a native program's stderr into errors; gpg writes there.
        $saved = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            $status = & $gpg.Source --batch --status-fd 1 --verify $sums 2>$null
        } finally {
            $ErrorActionPreference = $saved
        }
        if ($status -match "^\[GNUPG:\] VALIDSIG $rcloneKey ") {
            Write-Host "SHA256SUMS: good signature from rclone's key $rcloneKey"
            return
        }
        if ($status -match '^\[GNUPG:\] (BADSIG|VALIDSIG)') {
            throw "SHA256SUMS: the signature is bad or not rclone's ($rcloneKey): refusing it"
        }
    }
    if ($env:RCLONE_REQUIRE_SIGNATURE -eq '1') {
        throw "SHA256SUMS: can't check its signature: needs gpg with rclone's key ($rcloneKey)"
    }
    Write-Warning "SHA256SUMS: signature not checked (gpg doesn't have rclone's key $rcloneKey); the SHA-256 below still is"
}

# Downloads the pinned rclone for the target into sidecars\bin\download\<version>\ (or reuses
# it), checks it, and returns the path of the rclone binary taken out of it.
function Get-PinnedRclone {
    $platform = Get-RclonePlatform $triple
    if (-not $platform) { throw "no rclone release is known for ${triple}: set RCLONE instead" }
    $name = "rclone-$version-$platform"
    $base = "https://downloads.rclone.org/$version"
    $dir = Join-Path (Join-Path $out 'download') $version
    $zip = Join-Path $dir "$name.zip"
    $sums = Join-Path $dir 'SHA256SUMS'
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    # Windows PowerShell 5.1 may still default to TLS 1.0.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    if (-not (Test-Path $sums)) {
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile "$sums.part"
        Move-Item -Force "$sums.part" $sums
    }
    try {
        Test-Signature $sums
    } catch {
        Remove-Item -Force $sums
        throw
    }
    $want = Get-ExpectedSha256 $sums "$name.zip"
    if (-not $want) { throw "SHA256SUMS of rclone $version has no $name.zip" }
    if (-not (Test-Path $zip)) {
        Write-Host "downloading $base/$name.zip"
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$name.zip" -OutFile "$zip.part"
        Move-Item -Force "$zip.part" $zip
    }
    $got = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
    if ($got -ne $want.ToLowerInvariant()) {
        Remove-Item -Force $zip
        throw "${name}.zip: SHA-256 $got, but SHA256SUMS says ${want}: deleted, not used"
    }
    Write-Host "${name}.zip: SHA-256 matches SHA256SUMS ($want)"
    # Only the one member is read out of the archive.
    $member = "$name/rclone$ext"
    $binDir = Join-Path $dir $platform
    New-Item -ItemType Directory -Force -Path $binDir | Out-Null
    $bin = Join-Path $binDir "rclone$ext"
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($zip)
    try {
        $entry = $archive.GetEntry($member)
        if (-not $entry) { throw "${name}.zip has no $member" }
        [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $bin, $true)
    } finally {
        $archive.Dispose()
    }
    return $bin
}

$rclone = $null
if ($env:RCLONE_DOWNLOAD -eq '1') {
    $rclone = Get-PinnedRclone
} elseif ($env:RCLONE) {
    $rclone = $env:RCLONE
} elseif (Get-Command rclone -ErrorAction SilentlyContinue) {
    $rclone = (Get-Command rclone).Source
}
if (-not $rclone -or -not (Test-Path -PathType Leaf $rclone)) {
    throw "rclone $version not found: set `$env:RCLONE_DOWNLOAD = '1' to download it (checked against its SHA256SUMS), or `$env:RCLONE to rclone.exe (https://downloads.rclone.org/$version/)"
}
if ($triple -eq $hostTriple) {
    $found = ((& $rclone version) | Select-Object -First 1) -replace '^rclone ', ''
    if ($found -ne $version) {
        throw "$rclone is rclone $found, not the pinned $version (sidecars\RCLONE_VERSION): set `$env:RCLONE_DOWNLOAD = '1', or `$env:RCLONE to rclone $version"
    }
}
$dest = Join-Path $out "filebridge-rclone-$triple$ext"
Copy-Item -Force $rclone $dest
Write-Host "sidecar: $dest ($version, from $rclone)"
