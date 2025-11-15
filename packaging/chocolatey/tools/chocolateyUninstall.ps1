$ErrorActionPreference = 'Stop'

$packageName = 'anv'
$toolsDir    = Split-Path -Parent $MyInvocation.MyCommand.Definition
$binDir      = Join-Path $toolsDir 'bin'

if (Test-Path $binDir) {
    Remove-Item $binDir -Recurse -Force -ErrorAction SilentlyContinue
}

Uninstall-ChocolateyPath $binDir -PathType 'Machine' -ErrorAction SilentlyContinue
