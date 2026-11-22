param([Parameter(Mandatory=$true)][string]$Run)
try {
    $root=Split-Path -Parent $PSScriptRoot
    . (Join-Path $root 'scripts/release-source.ps1')
    $current=Get-ReleaseSourceDigest $root
    $staged=Get-ReleaseSourceDigest (Join-Path $Run 'source')
    if ($current.digest -cne $staged.digest) { throw 'source_changed' }
    Write-Output 'source_matches=true'; exit 0
} catch { Write-Output 'source_matches=false'; exit 1 }
