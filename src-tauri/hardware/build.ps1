$ErrorActionPreference = 'Stop'
$hardwareRoot = $PSScriptRoot
$vendorDir = Join-Path $hardwareRoot 'vendor'
$runtimeDir = Join-Path $hardwareRoot 'runtime'
New-Item -ItemType Directory -Force -Path $runtimeDir | Out-Null

# Validate all pinned upstream files before building or reusing a helper.
$manifest = Get-Content -LiteralPath (Join-Path $vendorDir 'checksums.json') -Raw | ConvertFrom-Json
foreach ($entry in $manifest.PSObject.Properties) {
    $path = Join-Path $vendorDir $entry.Name
    if (!(Test-Path -LiteralPath $path) -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $entry.Value) {
        throw "Missing or modified sensor dependency: $($entry.Name)"
    }
}
foreach ($file in Get-ChildItem -LiteralPath $vendorDir -Filter '*.dll') {
    $destination = Join-Path $runtimeDir $file.Name
    if (!(Test-Path -LiteralPath $destination) -or (Get-FileHash -LiteralPath $file.FullName).Hash -ne (Get-FileHash -LiteralPath $destination).Hash) {
        Copy-Item -LiteralPath $file.FullName -Destination $destination -Force
    }
}
Copy-Item -LiteralPath (Join-Path $vendorDir 'LibreHardwareMonitor.exe.config') -Destination (Join-Path $runtimeDir 'WinMeter.Sensors.exe.config') -Force
# Keep notices in source control and include them in every binary distribution.
Copy-Item -Path (Join-Path $hardwareRoot 'licenses/*.txt') -Destination $runtimeDir -Force
$compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
if (!(Test-Path -LiteralPath $compiler)) { throw '.NET Framework C# compiler not found. Build on Windows x64 with .NET Framework 4.7.2 or later.' }
$binary = Join-Path $runtimeDir 'WinMeter.Sensors.exe'
$source = Join-Path $hardwareRoot 'Program.cs'
$serviceSource = Join-Path $hardwareRoot 'Service.cs'
$inputs = @($source, $serviceSource, $PSCommandPath) + @(Get-ChildItem -LiteralPath $vendorDir -File | Select-Object -ExpandProperty FullName)
$latestInput = ($inputs | Get-Item | Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1).LastWriteTimeUtc
if ((Test-Path -LiteralPath $binary) -and (Get-Item -LiteralPath $binary).LastWriteTimeUtc -ge $latestInput) { exit 0 }
& $compiler /nologo /target:winexe /platform:x64 /optimize+ "/out:$binary" "/reference:$runtimeDir/LibreHardwareMonitorLib.dll" "/reference:$runtimeDir/DiskInfoToolkit.dll" /reference:System.Web.Extensions.dll /reference:System.ServiceProcess.dll $source $serviceSource
if ($LASTEXITCODE -ne 0) { throw 'Sensor helper compilation failed' }
