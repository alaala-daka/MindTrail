$ErrorActionPreference='Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class ReleaseSourceIdentity {
 [StructLayout(LayoutKind.Sequential)] public struct Info { public uint Attributes, CreationLow, CreationHigh, AccessLow, AccessHigh, WriteLow, WriteHigh, Volume, SizeHigh, SizeLow, Links, IndexHigh, IndexLow; }
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] public static extern SafeFileHandle CreateFileW(string path,uint access,uint share,IntPtr security,uint creation,uint flags,IntPtr template);
 [DllImport("kernel32.dll",SetLastError=true)] [return:MarshalAs(UnmanagedType.Bool)] public static extern bool GetFileInformationByHandle(SafeFileHandle handle,out Info info);
}
'@
function Get-SourceIdentity([string]$Path) {
    $handle=[ReleaseSourceIdentity]::CreateFileW($Path,0x80,7,[IntPtr]::Zero,3,0x00200000,[IntPtr]::Zero)
    try {
        if ($handle.IsInvalid) { throw 'source_open_failed' }
        $info=[ReleaseSourceIdentity+Info]::new()
        if (-not [ReleaseSourceIdentity]::GetFileInformationByHandle($handle,[ref]$info) -or $info.Links -ne 1 -or ($info.Attributes -band 0x400)) { throw 'unsafe_source_file' }
        return ('{0}:{1}:{2}:{3}:{4}' -f $info.Volume,$info.IndexHigh,$info.IndexLow,$info.SizeHigh,$info.SizeLow)
    } finally { $handle.Dispose() }
}

function Get-ReleaseSourceFiles([string]$Root) {
    $rootItem=Get-Item -LiteralPath $Root -Force
    if (-not $rootItem.PSIsContainer -or ($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_source_root' }
    $fixed=@('package.json','package-lock.json','index.html','vite.config.ts','tsconfig.json','tsconfig.node.json','src-tauri/Cargo.toml','src-tauri/Cargo.lock','src-tauri/build.rs','src-tauri/tauri.conf.json','scripts/release-lib.ps1','scripts/release-source.ps1','scripts/release-source-verify.ps1','scripts/release-environment.ps1','scripts/release-cargo-wrapper.cs','scripts/release-cargo-preflight.cs','scripts/release_cargo_policy.py','scripts/release_dependencies.py','scripts/release_environment.py','scripts/release_job.py','scripts/release_launcher.py','scripts/release_nsis_extract.py','scripts/release_nsis_readiness.py','scripts/release_producer_build.py','scripts/release_tools.py','scripts/prepare-producer.ps1','scripts/release-build-hook.ps1','scripts/prepare-release-stage.ps1','scripts/verify-release.ps1')
    $optional=@('tsconfig.app.json','scripts/build-release.ps1','scripts/check-webview.ps1')
    $directories=@('src','src-tauri/src','src-tauri/icons','src-tauri/capabilities','protocol','public')
    $extensions=@('.ts','.tsx','.css','.svg','.png','.jpg','.jpeg','.webp','.woff','.woff2','.ico','.icns','.json','.rs','.md','.html')
    foreach ($parent in @('src-tauri','scripts')) {
        $item=Get-Item -LiteralPath (Join-Path $rootItem.FullName $parent) -Force
        if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_source_directory' }
    }
    $result=[Collections.Generic.List[string]]::new()
    foreach ($name in ($fixed + $optional)) {
        $path=Join-Path $rootItem.FullName $name
        if (Test-Path -LiteralPath $path) {
            $entry=Get-Item -LiteralPath $path -Force
            if ($entry.PSIsContainer -or ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_source_file' }
            $result.Add($name)
        } elseif ($name -notin $optional) { throw 'missing_source_file' }
    }
    foreach ($name in $directories) {
        $dir=Join-Path $rootItem.FullName $name
        if (-not (Test-Path -LiteralPath $dir)) { if ($name -in @('public','protocol','src-tauri/capabilities')) { continue }; throw 'missing_source_directory' }
        $entry=Get-Item -LiteralPath $dir -Force
        if (-not $entry.PSIsContainer -or ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_source_directory' }
        foreach ($item in Get-ChildItem -LiteralPath $dir -Force -Recurse) {
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'source_link' }
            if ($item.Name.StartsWith('.')) { throw 'unapproved_source_file' }
            if (-not $item.PSIsContainer) {
                $relative=[IO.Path]::GetRelativePath($rootItem.FullName,$item.FullName).Replace('\','/')
                if ($relative.StartsWith('../') -or $relative -match '[\x00-\x1f]') { throw 'invalid_source_path' }
                if ($item.Name.StartsWith('.') -or $item.Extension.ToLowerInvariant() -notin $extensions -or $item.Name -match '(?i)(^|[._-])(secret|credential|private|token|apikey|api_key)([._-]|$)') { throw 'unapproved_source_file' }
                $result.Add($relative)
            }
        }
    }
    $items=[string[]]$result.ToArray(); [Array]::Sort($items,[StringComparer]::Ordinal)
    return ,$items
}

function Assert-ReleaseSnapshotsEqual([string]$Before,[string]$Staged,[string]$After) {
    if ($Staged -cne $Before -or $After -cne $Before) { throw 'source_changed' }
}

function Get-ReleaseSourceDigest([string]$Root) {
    $files=Get-ReleaseSourceFiles $Root
    $hash=[Security.Cryptography.IncrementalHash]::CreateHash([Security.Cryptography.HashAlgorithmName]::SHA256)
    try {
        foreach ($name in $files) {
            $path=Join-Path $Root $name
            $entry=Get-Item -LiteralPath $path -Force
            if ($entry.PSIsContainer -or ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $entry.Length -gt 33554432) { throw 'invalid_source_file' }
            $identity=Get-SourceIdentity $entry.FullName
            $bytes=[IO.File]::ReadAllBytes($entry.FullName)
            if ($bytes.LongLength -ne $entry.Length -or (Get-SourceIdentity $entry.FullName) -cne $identity) { throw 'source_changed' }
            $header=[Text.Encoding]::UTF8.GetBytes(($name + [char]0 + $bytes.LongLength.ToString([Globalization.CultureInfo]::InvariantCulture) + [char]0))
            $hash.AppendData($header); $hash.AppendData($bytes)
        }
        $digest=[Convert]::ToHexString($hash.GetHashAndReset()).ToLowerInvariant()
        return @{ digest=$digest; files=$files }
    } finally { $hash.Dispose() }
}
