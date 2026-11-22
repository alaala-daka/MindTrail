$ErrorActionPreference='Stop'
# Disabled until the complete build chain has an independently reviewed launcher.
throw 'release_environment_not_admitted'
$root=Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root
& pwsh -NoProfile -File (Join-Path $PSScriptRoot 'prepare-producer.ps1')
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& npm.cmd run build
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
exit 0
