[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
# Disabled until the reviewed release launcher supplies an isolated environment.
throw 'release_environment_not_admitted'
try {
    . (Join-Path $PSScriptRoot 'release-lib.ps1')
    $workspace=[IO.Path]::GetFullPath($env:MINDTRAIL_RELEASE_WORKSPACE)
    $source=[IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
    $run=Split-Path -Parent $source
    $base=Join-Path $workspace '.tools/productization-release'
    $runItem=Get-Item -LiteralPath $run -Force
    if ($runItem.Name -cnotmatch '^run-[a-f0-9]{32}$' -or $runItem.Parent.FullName -cne $base -or $source -cne (Join-Path $run 'source')) { throw 'invalid_stage' }
    foreach ($path in @($workspace,(Join-Path $workspace '.tools'),$base,$run,$source,(Join-Path $run 'target'))) {
        $item=Get-Item -LiteralPath $path -Force
        if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_stage' }
    }
    $owner=Get-Content -LiteralPath (Join-Path $run 'owner') -Raw -Encoding ASCII
    if ($owner -cne $runItem.Name.Substring(4)) { throw 'invalid_stage' }
    $cargo=Join-Path $workspace '.tools/cargo/bin/cargo.exe'
    $env:CARGO_HOME=Join-Path $workspace '.tools/cargo'
    $env:RUSTUP_HOME=Join-Path $workspace '.tools/rustup'
    $env:CARGO_TARGET_DIR=Join-Path $run 'target'
    Remove-Item Env:MINDTRAIL_AI_BASE_URL,Env:MINDTRAIL_AI_MODEL,Env:MINDTRAIL_AI_API_KEY -ErrorAction SilentlyContinue
    & $cargo build --locked --offline --release --target x86_64-pc-windows-msvc --features producer --bin mindtrail-producer --manifest-path (Join-Path $source 'src-tauri/Cargo.toml')
    $buildExit=$LASTEXITCODE
    if ($buildExit -ne 0) { throw 'producer_build_failed' }
    $built=Join-Path $run 'target/x86_64-pc-windows-msvc/release/mindtrail-producer.exe'
    $resource=Join-Path $source 'src-tauri/resources/producer'
    if (-not (Test-Path -LiteralPath $resource)) { New-Item -ItemType Directory -Path $resource | Out-Null }
    $digest=Copy-ProducerAfterBuild $buildExit $built (Join-Path $resource 'mindtrail-producer.exe')
    $record=@{ target='x86_64-pc-windows-msvc'; features=@('producer'); sha256=$digest } | ConvertTo-Json -Compress
    $stream=[IO.File]::Open((Join-Path $run 'producer-build.json'),[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    try { $bytes=[Text.Encoding]::UTF8.GetBytes($record); $stream.Write($bytes,0,$bytes.Length) } finally { $stream.Dispose() }
    Write-Output 'producer_prepared'
    exit 0
} catch { Write-Output 'producer_prepare_failed'; exit 1 }
