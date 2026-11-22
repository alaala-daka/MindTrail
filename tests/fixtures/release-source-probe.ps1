param([string]$Library,[string]$Root,[switch]$Mismatch)
try {
    . $Library
    if ($Mismatch) { Assert-ReleaseSnapshotsEqual 'before' 'before' 'after'; exit 0 }
    (Get-ReleaseSourceDigest $Root).digest
    exit 0
} catch { Write-Output 'source_validation_failed'; exit 1 }
