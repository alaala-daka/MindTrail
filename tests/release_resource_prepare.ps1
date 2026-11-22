param([Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{32}$')][string]$RunId)
$ErrorActionPreference='Stop'
function Get-ProducerIdentity([string]$Path) {
    $handle=[ReleaseSourceIdentity]::CreateFileW($Path,0x80,7,[IntPtr]::Zero,3,0x00200000,[IntPtr]::Zero)
    try {
        if ($handle.IsInvalid) { throw 'source_open_failed' }
        $info=[ReleaseSourceIdentity+Info]::new()
        if (-not [ReleaseSourceIdentity]::GetFileInformationByHandle($handle,[ref]$info) -or $info.Links -notin @(1,2) -or ($info.Attributes -band 0x400)) { throw 'unsafe_source_file' }
        if ($info.Links -eq 2) {
            $peer=Join-Path (Split-Path -Parent $Path) ('deps/' + [IO.Path]::GetFileNameWithoutExtension($Path).Replace('-','_') + '.exe')
            $peerItem=Get-Item -LiteralPath $peer -Force
            if ($peerItem.PSIsContainer -or ($peerItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'unsafe_source_file' }
            $peerHandle=[ReleaseSourceIdentity]::CreateFileW($peer,0x80,7,[IntPtr]::Zero,3,0x00200000,[IntPtr]::Zero)
            try {
                if ($peerHandle.IsInvalid) { throw 'source_open_failed' }
                $peerInfo=[ReleaseSourceIdentity+Info]::new()
                if (-not [ReleaseSourceIdentity]::GetFileInformationByHandle($peerHandle,[ref]$peerInfo) -or $peerInfo.Links -ne 2 -or $peerInfo.Volume -ne $info.Volume -or $peerInfo.IndexHigh -ne $info.IndexHigh -or $peerInfo.IndexLow -ne $info.IndexLow) { throw 'unsafe_source_file' }
            } finally { $peerHandle.Dispose() }
        }
        return ('{0}:{1}:{2}:{3}:{4}' -f $info.Volume,$info.IndexHigh,$info.IndexLow,$info.SizeHigh,$info.SizeLow)
    } finally { $handle.Dispose() }
}
$owned=$false; $mutated=$false
try {
    $workspace=Split-Path -Parent $PSScriptRoot
    . (Join-Path $workspace 'scripts/release-lib.ps1')
    . (Join-Path $workspace 'scripts/release-source.ps1')
    $run=Join-Path $workspace ('.tools/productization-release/run-'+$RunId)
    foreach ($path in @($workspace,(Join-Path $workspace '.tools'),(Join-Path $workspace '.tools/productization-release'),$run,(Join-Path $run 'source'),(Join-Path $run 'target'))) {
        $entry=Get-Item -LiteralPath $path -Force
        if (-not $entry.PSIsContainer -or ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_run' }
    }
    $owner=Join-Path $run 'owner'; $null=Get-SourceIdentity $owner
    if ((Get-Content -LiteralPath $owner -Raw -Encoding ASCII) -cne $RunId -or (Test-Path -LiteralPath (Join-Path $run '.release-quarantine'))) { throw 'invalid_run' }
    $owned=$true
    $current=Get-ReleaseSourceDigest $workspace; $staged=Get-ReleaseSourceDigest (Join-Path $run 'source')
    if ($current.digest -cne $staged.digest) { throw 'source_changed' }
    $recordPath=Join-Path $run 'producer-build.json'; $identity=Get-SourceIdentity $recordPath
    $record=Get-Content -LiteralPath $recordPath -Raw | ConvertFrom-Json -AsHashtable
    if ((Get-SourceIdentity $recordPath) -cne $identity) { throw 'invalid_record' }
    if ($record.target -cne 'x86_64-pc-windows-msvc' -or $record.features.Count -ne 1 -or $record.features[0] -cne 'producer' -or $record.sha256 -cnotmatch '^[a-f0-9]{64}$') { throw 'invalid_record' }
    $built=Join-Path $run 'target/x86_64-pc-windows-msvc/release/mindtrail-producer.exe'
    $binaryIdentity=Get-ProducerIdentity $built
    if ((Assert-ProducerBinary $built) -cne $record.sha256) { throw 'producer_changed' }
    $resources=Join-Path $run 'source/src-tauri/resources'
    if (Test-Path -LiteralPath $resources) { throw 'resource_already_exists' }
    $mutated=$true
    New-Item -ItemType Directory -Path (Join-Path $resources 'producer') -Force | Out-Null
    $destination=Join-Path $resources 'producer/mindtrail-producer.exe'
    $digest=Copy-ProducerAfterBuild 0 $built $destination
    if ($digest -cne $record.sha256 -or (Get-ProducerIdentity $built) -cne $binaryIdentity) { throw 'producer_changed' }
    $resourceRecord=@{sha256=$digest;target=$record.target} | ConvertTo-Json -Compress
    $stream=[IO.File]::Open((Join-Path $run 'producer-resource.json'),[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    try { $bytes=[Text.Encoding]::UTF8.GetBytes($resourceRecord); $stream.Write($bytes,0,$bytes.Length) } finally { $stream.Dispose() }
    Write-Output 'producer_resource_prepared'; exit 0
} catch {
    if ($owned -and $mutated) {
        try { $marker=[IO.File]::Open((Join-Path $run '.release-quarantine'),[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None); $marker.Dispose() } catch { }
    }
    Write-Output 'producer_resource_failed'; exit 1
}
