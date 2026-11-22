param([string]$Library,[string]$SourceLibrary,[string]$Root,[switch]$Child,[switch]$TimeoutTest,[switch]$WaitChild)
if ($WaitChild) { Start-Sleep -Seconds 30; exit 0 }
if ($Child) {
    if ($env:SYNTHETIC_CREDENTIAL -or $env:CARGO_BUILD_RUSTFLAGS -or $env:NODE_OPTIONS -or $env:NPM_CONFIG_REGISTRY) { exit 2 }
    if (-not $env:CARGO_HOME -or -not $env:NPM_CONFIG_USERCONFIG -or -not $env:NPM_CONFIG_GLOBALCONFIG) { exit 3 }
    if ($env:CARGO_NET_OFFLINE -cne 'true' -or $env:NPM_CONFIG_OFFLINE -cne 'true') { exit 4 }
    exit 0
}
try {
    . $SourceLibrary
    . $Library
    $original=${function:Assert-ReleaseNoAncestorConfig}
    function Assert-ReleaseNoAncestorConfig([string]$Directory) { & $original $Directory $Root }
    $env:SYNTHETIC_CREDENTIAL='SYNTHETIC_PRIVATE_SENTINEL'
    $env:CARGO_BUILD_RUSTFLAGS='SYNTHETIC_PRIVATE_SENTINEL'
    $env:NODE_OPTIONS='SYNTHETIC_PRIVATE_SENTINEL'
    $env:NPM_CONFIG_REGISTRY='SYNTHETIC_PRIVATE_SENTINEL'
    $childEnv=New-ReleaseChildEnvironment $Root (Join-Path $Root 'source') $env:SystemRoot (Join-Path $env:SystemRoot 'System32') (Join-Path $Root 'rustup')
    if ($TimeoutTest) {
        try { $null=Invoke-ReleaseChild (Join-Path $PSHOME 'pwsh.exe') @('-NoProfile','-File',$PSCommandPath,'-WaitChild') (Join-Path $Root 'source') $childEnv 500; exit 3 }
        catch { if ($_.Exception.Message -cne 'child_timed_out') { throw }; Write-Output 'timeout_reaped'; exit 0 }
    }
    $code=Invoke-ReleaseChild (Join-Path $PSHOME 'pwsh.exe') @('-NoProfile','-File',$PSCommandPath,'-Child') (Join-Path $Root 'source') $childEnv
    if ($code -ne 0) { throw 'child_failed' }
    Write-Output 'isolated_child_passed'
    exit 0
} catch { Write-Output 'environment_validation_failed'; exit 1 }
