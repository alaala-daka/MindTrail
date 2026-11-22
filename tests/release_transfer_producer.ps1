param([Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{32}$')][string]$DestinationRun)
$ErrorActionPreference='Stop'; $mutated=$false; $owned=$false
try {
    $workspace=Split-Path -Parent $PSScriptRoot
    . (Join-Path $workspace 'scripts/release-lib.ps1')
    . (Join-Path $workspace 'scripts/release-source.ps1')
    $base=Join-Path $workspace '.tools/productization-release'
    $source=Join-Path $base 'run-02bb4e8bbebd45da906552912bc6cb2a'
    $dest=Join-Path $base ('run-'+$DestinationRun)
    foreach($path in @($workspace,(Join-Path $workspace '.tools'),$base,$source,$dest,(Join-Path $source 'source'),(Join-Path $dest 'source'),(Join-Path $dest 'target'),(Join-Path $source 'target'),(Join-Path $source 'target/x86_64-pc-windows-msvc'),(Join-Path $source 'target/x86_64-pc-windows-msvc/release'))) {
        $item=Get-Item -LiteralPath $path -Force
        if(-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_run' }
    }
    foreach($entry in @(@($source,'02bb4e8bbebd45da906552912bc6cb2a'),@($dest,$DestinationRun))) {
        $owner=Join-Path $entry[0] 'owner'; $null=Get-SourceIdentity $owner
        if((Get-Content -LiteralPath $owner -Raw -Encoding ASCII) -cne $entry[1]) { throw 'invalid_run' }
    }
    if(Test-Path -LiteralPath (Join-Path $dest '.release-quarantine')) { throw 'invalid_run' }; $owned=$true
    $expected='240b2bb2b6967bc41f0960947b9ed8661eb0d61e05110ad9269352bded2b16ee'
    $recordPath=Join-Path $source 'producer-build.json'; $recordId=Get-SourceIdentity $recordPath
    $record=Get-Content -LiteralPath $recordPath -Raw | ConvertFrom-Json -AsHashtable
    $diagPath=Join-Path $source 'producer-diagnostic.json'; $diagId=Get-SourceIdentity $diagPath
    $diag=Get-Content -LiteralPath $diagPath -Raw | ConvertFrom-Json -AsHashtable
    if((Get-SourceIdentity $recordPath) -cne $recordId -or (Get-SourceIdentity $diagPath) -cne $diagId) { throw 'record_changed' }
    if($record.sha256 -cne $expected -or $record.target -cne 'x86_64-pc-windows-msvc' -or $record.features.Count -ne 1 -or $record.features[0] -cne 'producer' -or $diag.exit_code -ne 0 -or $diag.timed_out -ne $false) { throw 'invalid_producer_record' }
    $current=Get-ReleaseSourceDigest $workspace; $old=Get-ReleaseSourceDigest (Join-Path $source 'source'); $new=Get-ReleaseSourceDigest (Join-Path $dest 'source')
    if($current.digest -cne $old.digest -or $current.digest -cne $new.digest) { throw 'source_changed' }
    $built=Join-Path $source 'target/x86_64-pc-windows-msvc/release/mindtrail-producer.exe'; $identity=Get-SourceIdentity $built
    if((Assert-ProducerBinary $built) -cne $expected) { throw 'producer_changed' }
    $output=Join-Path $dest 'target/x86_64-pc-windows-msvc/release'; if(Test-Path -LiteralPath $output) { throw 'target_exists' }
    $mutated=$true; New-Item -ItemType Directory -Path $output -Force | Out-Null
    if((Copy-ProducerAfterBuild 0 $built (Join-Path $output 'mindtrail-producer.exe')) -cne $expected -or (Get-SourceIdentity $built) -cne $identity) { throw 'producer_changed' }
    foreach($name in @('producer-build.json','producer-diagnostic.json','producer-toolchain-inspection-v2.json')) {
        $from=Join-Path $source $name; $to=Join-Path $dest $name; $id=Get-SourceIdentity $from
        [IO.File]::Copy($from,$to,$false)
        if((Get-SourceIdentity $from) -cne $id -or (Get-FileHash -LiteralPath $from).Hash -cne (Get-FileHash -LiteralPath $to).Hash) { throw 'record_changed' }
    }
    Write-Output 'producer_transfer=passed'; exit 0
} catch {
    if($owned -and $mutated) { try { $f=[IO.File]::Open((Join-Path $dest '.release-quarantine'),[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None); $f.Dispose() } catch {} }
    Write-Output 'producer_transfer=failed'; exit 1
}
