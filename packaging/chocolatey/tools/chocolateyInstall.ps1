$ErrorActionPreference = 'Stop'

$packageName = 'anv'
$toolsDir     = Split-Path -Parent $MyInvocation.MyCommand.Definition
$version      = '0.1.0'
$archiveName  = "anv-$version-x86_64-pc-windows-msvc.zip"

$downloadUrl  = "https://github.com/Vedant-Asati03/anv/releases/download/v$version/$archiveName"
$archivePath  = Join-Path $toolsDir $archiveName
$checksum     = 'e1e22a53224e3a503602c95b6cb0012ce5a74951d57ce69e835d7a53e79ed1b5'
$checksumType = 'sha256'
$binDir       = Join-Path $toolsDir 'bin'

if (-not (Test-Path $binDir)) {
    New-Item -ItemType Directory -Path $binDir | Out-Null
}

Get-ChocolateyWebFile -PackageName $packageName `
    -FileFullPath $archivePath `
    -Url $downloadUrl `
    -Checksum $checksum `
    -ChecksumType $checksumType

Get-ChocolateyUnzip -FileFullPath $archivePath -Destination $binDir

Remove-Item $archivePath -Force

$exePath = Join-Path $binDir 'anv.exe'
if (-not (Test-Path $exePath)) {
    throw "Expected anv.exe in archive but it was not found."
}

Install-ChocolateyPath $binDir -PathType 'Machine'
