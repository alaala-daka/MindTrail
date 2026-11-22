$ErrorActionPreference='Stop'

# Inspect names and metadata only; never open configuration in ancestor directories.
function Assert-ReleaseNoAncestorConfig([string]$Directory,[string]$TestStopAt='') {
    $current=Get-Item -LiteralPath $Directory -Force
    while ($null -ne $current) {
        if ($current -isnot [IO.DirectoryInfo] -or ($current.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'unsafe_ancestor' }
        foreach ($name in @('.npmrc','.cargo/config','.cargo/config.toml')) {
            if (Test-Path -LiteralPath (Join-Path $current.FullName $name)) { throw 'ancestor_config_present' }
        }
        if ($TestStopAt -and $current.FullName -ceq $TestStopAt) { break }
        $current=$current.Parent
    }
}

function New-ReleaseChildEnvironment([string]$Run,[string]$Source,[string]$SystemRoot,[string]$Path,[string]$RustupHome) {
    Assert-ReleaseNoAncestorConfig $Source
    foreach ($name in @('temp','cargo-home','empty-home','npm-cache')) {
        $item=Get-Item -LiteralPath (Join-Path $Run $name) -Force
        if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'unsafe_environment_directory' }
    }
    # Cargo home may later be populated with the reviewed offline registry only.
    foreach ($item in Get-ChildItem -LiteralPath (Join-Path $Run 'cargo-home') -Force) {
        if ($item.Name -cne 'registry' -or -not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'unsafe_cargo_home' }
    }
    foreach ($name in @('npm-user.conf','npm-global.conf','git-global.conf')) {
        $p=Join-Path $Run $name
        $info=Get-Item -LiteralPath $p -Force
        if ($info.PSIsContainer -or $info.Length -ne 0 -or ($info.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'unsafe_environment_file' }
        $null=Get-SourceIdentity $p
    }
    return @{
        SystemRoot=$SystemRoot; WINDIR=$SystemRoot; ComSpec=(Join-Path $SystemRoot 'System32/cmd.exe'); PATH=$Path
        TEMP=(Join-Path $Run 'temp'); TMP=(Join-Path $Run 'temp'); USERPROFILE=(Join-Path $Run 'empty-home')
        HOME=(Join-Path $Run 'empty-home'); CARGO_HOME=(Join-Path $Run 'cargo-home'); RUSTUP_HOME=$RustupHome
        CARGO_TARGET_DIR=(Join-Path $Run 'target'); CARGO_NET_OFFLINE='true'; CARGO_INCREMENTAL='0'
        NPM_CONFIG_USERCONFIG=(Join-Path $Run 'npm-user.conf'); NPM_CONFIG_GLOBALCONFIG=(Join-Path $Run 'npm-global.conf')
        NPM_CONFIG_CACHE=(Join-Path $Run 'npm-cache'); NPM_CONFIG_OFFLINE='true'
        GIT_CONFIG_NOSYSTEM='1'; GIT_CONFIG_GLOBAL=(Join-Path $Run 'git-global.conf'); GIT_TERMINAL_PROMPT='0'
    }
}

function Invoke-ReleaseChild([string]$Executable,[string[]]$Arguments,[string]$WorkingDirectory,[hashtable]$Environment,[int]$TimeoutMilliseconds=30000) {
    if ($TimeoutMilliseconds -lt 1 -or $TimeoutMilliseconds -gt 3600000) { throw 'invalid_child_timeout' }
    $start=[Diagnostics.ProcessStartInfo]::new()
    $start.FileName=$Executable; $start.WorkingDirectory=$WorkingDirectory; $start.UseShellExecute=$false
    foreach ($arg in $Arguments) { $null=$start.ArgumentList.Add($arg) }
    $start.Environment.Clear()
    foreach ($key in $Environment.Keys) { $start.Environment[$key]=[string]$Environment[$key] }
    $process=[Diagnostics.Process]::Start($start)
    try {
        if (-not $process.WaitForExit($TimeoutMilliseconds)) {
            $process.Kill($true)
            if (-not $process.WaitForExit(5000)) { throw 'child_cleanup_failed' }
            throw 'child_timed_out'
        }
        return $process.ExitCode
    } finally { $process.Dispose() }
}
