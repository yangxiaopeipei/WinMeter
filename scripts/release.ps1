param([switch]$PackageOnly)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $package = Get-Content package.json -Raw | ConvertFrom-Json
    $version = $package.version
    if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Expected a stable semantic version.' }
    $tauri = Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json
    # Node handles the empty-string key in package-lock.json on Windows PowerShell 5.1.
    $lockVersion = & node scripts/check-version.mjs
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read package lock version.' }
    $cargo = Get-Content src-tauri/Cargo.toml -Raw
    $cargoVersion = [regex]::Match($cargo, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
    $cargoLock = Get-Content src-tauri/Cargo.lock -Raw
    $cargoLockVersion = [regex]::Match($cargoLock, '(?m)^name = "winmeter"\r?\nversion = "([^"]+)"').Groups[1].Value
    foreach ($other in @($tauri.version, $lockVersion, $cargoVersion, $cargoLockVersion)) {
        if ($other -ne $version) { throw "Version mismatch: expected $version, found $other" }
    }
    if (!$PackageOnly) {
        & npm.cmd test
        if ($LASTEXITCODE -ne 0) { throw 'Frontend tests failed.' }
        & cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
        if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed.' }
        & npm.cmd run build
        if ($LASTEXITCODE -ne 0) { throw 'Installer build failed.' }
    }

    $release = Join-Path $root 'src-tauri/target/release'
    $bundle = Join-Path $release 'bundle/nsis'
    $installer = @(Get-ChildItem -LiteralPath $bundle -Filter "*_${version}_x64-setup.exe")
    if ($installer.Count -ne 1) { throw 'Expected exactly one x64 installer for this version.' }
    $required = @('WinMeter.Sensors.exe', 'WinMeter.Sensors.exe.config', 'LibreHardwareMonitorLib.dll', 'DiskInfoToolkit.dll', 'LibreHardwareMonitor-LICENSE.txt', 'THIRD-PARTY-NOTICES.txt')
    foreach ($file in $required) {
        if (!(Test-Path -LiteralPath (Join-Path $release "hardware/$file"))) { throw "Missing runtime resource: $file" }
    }
    $dist = Join-Path $root 'dist'
    New-Item -ItemType Directory -Path $dist -Force | Out-Null
    $stem = "WinMeter-$version-windows-x64"
    Copy-Item -LiteralPath $installer[0].FullName -Destination (Join-Path $dist "$stem-setup.exe") -Force
    Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
    $portable = Join-Path $dist "$stem-portable.zip"
    if (Test-Path -LiteralPath $portable) { Remove-Item -LiteralPath $portable }
    $archive = [IO.Compression.ZipFile]::Open($portable, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($file in @('README.md', 'CHANGELOG.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'docs/RELEASING.md')) {
            [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, (Join-Path $root $file), $file, [IO.Compression.CompressionLevel]::Optimal) | Out-Null
        }
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, (Join-Path $release 'winmeter.exe'), 'winmeter.exe', [IO.Compression.CompressionLevel]::Optimal) | Out-Null
        foreach ($file in Get-ChildItem -LiteralPath (Join-Path $release 'hardware') -File) {
            [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file.FullName, "hardware/$($file.Name)", [IO.Compression.CompressionLevel]::Optimal) | Out-Null
        }
    } finally { $archive.Dispose() }
    $check = [IO.Compression.ZipFile]::OpenRead($portable)
    try {
        foreach ($file in $required) {
            if (!$check.GetEntry("hardware/$file")) { throw "Portable archive missing $file" }
        }
        if (!$check.GetEntry('winmeter.exe')) { throw 'Portable archive missing application.' }
    } finally { $check.Dispose() }
    $hashes = foreach ($file in @("$stem-setup.exe", "$stem-portable.zip")) {
        $hash = (Get-FileHash -LiteralPath (Join-Path $dist $file) -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $file"
    }
    [IO.File]::WriteAllLines((Join-Path $dist 'SHA256SUMS.txt'), [string[]]$hashes, (New-Object Text.UTF8Encoding($false)))
    Write-Host "Release $version ready: $dist"
} finally { Pop-Location }
