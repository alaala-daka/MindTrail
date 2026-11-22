$ErrorActionPreference='Stop'
try {
    . (Join-Path $PSScriptRoot 'release-source.ps1')
    $workspace=(Get-Item -LiteralPath (Split-Path -Parent $PSScriptRoot) -Force).FullName
    $tools=Get-Item -LiteralPath (Join-Path $workspace '.tools') -Force
    if (-not $tools.PSIsContainer -or ($tools.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_tools' }
    $base=Join-Path $tools.FullName 'productization-release'
    if (-not (Test-Path -LiteralPath $base)) {
        New-Item -ItemType Directory -Path $base | Out-Null
        $owner=[IO.File]::Open((Join-Path $base 'owner'),[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
        try { $bytes=[Text.Encoding]::ASCII.GetBytes('mindtrail-productization-release-v1'); $owner.Write($bytes,0,$bytes.Length) } finally { $owner.Dispose() }
    }
    $baseItem=Get-Item -LiteralPath $base -Force
    if (-not $baseItem.PSIsContainer -or ($baseItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_base' }
    $baseOwner=Join-Path $base 'owner'; $null=Get-SourceIdentity $baseOwner
    if ((Get-Content -LiteralPath $baseOwner -Raw -Encoding ASCII) -cne 'mindtrail-productization-release-v1') { throw 'invalid_base_owner' }
    $id=[guid]::NewGuid().ToString('N'); $run=Join-Path $base ('run-'+$id)
    New-Item -ItemType Directory -Path $run | Out-Null
    [IO.File]::WriteAllText((Join-Path $run 'owner'),$id,[Text.Encoding]::ASCII)
    foreach ($name in @('source','target','temp','release','cargo-home','empty-home','npm-cache')) { New-Item -ItemType Directory -Path (Join-Path $run $name) | Out-Null }
    foreach ($name in @('npm-user.conf','npm-global.conf','git-global.conf')) {
        $stream=[IO.File]::Open((Join-Path $run $name),[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
        $stream.Dispose()
    }
    $source=Join-Path $run 'source'
    $before=Get-ReleaseSourceDigest $workspace
    $entries=[Collections.Generic.List[object]]::new()
    foreach ($name in $before.files) {
        $from=Join-Path $workspace $name; $to=Join-Path $source $name
        $parent=Split-Path -Parent $to
        if (-not (Test-Path -LiteralPath $parent)) { [IO.Directory]::CreateDirectory($parent) | Out-Null }
        $idBefore=Get-SourceIdentity $from
        [IO.File]::Copy($from,$to,$false)
        if ((Get-SourceIdentity $from) -cne $idBefore) { throw 'source_changed' }
        $original=Get-Item -LiteralPath $from -Force; $copy=Get-Item -LiteralPath $to -Force
        $digest=(Get-FileHash -LiteralPath $from -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($original.Length -ne $copy.Length -or (Get-FileHash -LiteralPath $to -Algorithm SHA256).Hash.ToLowerInvariant() -cne $digest) { throw 'copy_mismatch' }
        $entries.Add(@{path=$name;size=$original.Length;sha256=$digest})
    }
    $staged=Get-ReleaseSourceDigest $source; $after=Get-ReleaseSourceDigest $workspace
    Assert-ReleaseSnapshotsEqual $before.digest $staged.digest $after.digest
    $head=(& git -C $workspace rev-parse HEAD); if ($LASTEXITCODE -ne 0 -or $head -cnotmatch '^[a-f0-9]{40}$') { throw 'git_failed' }
    $dirty=@(& git -C $workspace status --short --untracked-files=no); if ($LASTEXITCODE -ne 0) { throw 'git_failed' }
    $tracked=@(& git -C $workspace -c core.quotepath=false ls-files); if ($LASTEXITCODE -ne 0) { throw 'git_failed' }
    $known=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($name in $tracked) { $null=$known.Add($name) }
    $untracked=@($before.files | Where-Object { -not $known.Contains($_) })
    $manifest=@{ source_commit=$head; source_dirty=($dirty.Count -gt 0 -or $untracked.Count -gt 0); source_digest=$before.digest; tracked_dirty=$dirty; source_untracked=$untracked; files=$entries.ToArray(); created_utc=[DateTime]::UtcNow.ToString('o') }
    $json=$manifest | ConvertTo-Json -Depth 8
    [IO.File]::WriteAllText((Join-Path $run 'source-inputs.json'),$json,[Text.UTF8Encoding]::new($false))
    Write-Output 'stage_ready=true'
    Write-Output ('run=' + $id)
    exit 0
} catch { Write-Output 'stage_prepare_failed'; exit 1 }
