function Assert-ProducerBinary([string]$Path) {
    $item=Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $item.Length -lt 256 -or $item.Length -gt 268435456) { throw 'invalid_producer' }
    $bytes=[IO.File]::ReadAllBytes($item.FullName)
    if ($bytes[0] -ne 0x4d -or $bytes[1] -ne 0x5a) { throw 'invalid_pe' }
    $offset=[BitConverter]::ToUInt32($bytes,0x3c)
    if ($offset -gt ($bytes.Length-6) -or [BitConverter]::ToUInt32($bytes,[int]$offset) -ne 0x4550 -or [BitConverter]::ToUInt16($bytes,[int]$offset+4) -ne 0x8664) { throw 'invalid_pe' }
    $text=[Text.Encoding]::ASCII.GetString($bytes)
    if ($text.Contains('MINDTRAIL_TEST_ROOT') -or $text.Contains('MINDTRAIL_TEST_OUTPIPE')) { throw 'fixture_binary' }
    return (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
}
function Copy-ProducerAfterBuild([int]$BuildExitCode,[string]$BuiltPath,[string]$Destination) {
    if ($BuildExitCode -ne 0) { throw 'producer_build_failed' }
    $digest=Assert-ProducerBinary $BuiltPath
    $parent=Get-Item -LiteralPath (Split-Path -Parent $Destination) -Force
    if (-not $parent.PSIsContainer -or ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_resource_dir' }
    if (Test-Path -LiteralPath $Destination) { $old=Get-Item -LiteralPath $Destination -Force; if ($old.PSIsContainer -or ($old.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_destination' } }
    $temp=Join-Path $parent.FullName ('.producer-' + [guid]::NewGuid().ToString('N') + '.tmp')
    try {
        [IO.File]::Copy($BuiltPath,$temp,$false)
        if ((Assert-ProducerBinary $temp) -cne $digest) { throw 'copy_mismatch' }
        [IO.File]::Move($temp,$Destination,$true)
    } finally { if (Test-Path -LiteralPath $temp -PathType Leaf) { Remove-Item -LiteralPath $temp -Force } }
    return $digest
}
