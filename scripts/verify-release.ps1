[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$ReleaseDirectory)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class ReleaseFileIdentity {
  [StructLayout(LayoutKind.Sequential)] public struct Info {
    public uint Attributes, CreationLow, CreationHigh, AccessLow, AccessHigh, WriteLow, WriteHigh;
    public uint Volume, SizeHigh, SizeLow, Links, IndexHigh, IndexLow;
  }
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  public static extern SafeFileHandle CreateFileW(string path, uint access, uint share, IntPtr security, uint creation, uint flags, IntPtr template);
  [DllImport("kernel32.dll", SetLastError=true)]
  [return: MarshalAs(UnmanagedType.Bool)] public static extern bool GetFileInformationByHandle(SafeFileHandle handle, out Info info);
}
'@
function Get-ReleaseFileIdentity([string]$Path) {
    $handle=[ReleaseFileIdentity]::CreateFileW($Path,0x80,7,[IntPtr]::Zero,3,0x00200000,[IntPtr]::Zero)
    try {
        if ($handle.IsInvalid) { throw 'file_open_failed' }
        $info=[ReleaseFileIdentity+Info]::new()
        if (-not [ReleaseFileIdentity]::GetFileInformationByHandle($handle,[ref]$info) -or $info.Links -ne 1 -or ($info.Attributes -band 0x400)) { throw 'unsafe_file' }
        return ('{0}:{1}:{2}:{3}:{4}' -f $info.Volume,$info.IndexHigh,$info.IndexLow,$info.SizeHigh,$info.SizeLow)
    } finally { $handle.Dispose() }
}
function Assert-UniqueJson([System.Text.Json.JsonElement]$Element) {
    if ($Element.ValueKind -eq [System.Text.Json.JsonValueKind]::Object) {
        $keys=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        foreach ($property in $Element.EnumerateObject()) {
            if (-not $keys.Add($property.Name)) { throw 'duplicate_key' }
            Assert-UniqueJson $property.Value
        }
    } elseif ($Element.ValueKind -eq [System.Text.Json.JsonValueKind]::Array) {
        foreach ($child in $Element.EnumerateArray()) { Assert-UniqueJson $child }
    }
}
try {
    $root = Get-Item -LiteralPath $ReleaseDirectory -Force
    if (-not $root.PSIsContainer -or ($root.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_root' }
    for ($ancestor = $root.Parent; $null -ne $ancestor; $ancestor = $ancestor.Parent) { if ($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'invalid_root' } }
    $rootPath = $root.FullName
    $manifestPath = Join-Path $rootPath 'manifest.json'
    $manifestItem = Get-Item -LiteralPath $manifestPath -Force
    if ($manifestItem.PSIsContainer -or ($manifestItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $manifestItem.Length -gt 1048576) { throw 'invalid_manifest' }
    $manifestIdentity=Get-ReleaseFileIdentity $manifestPath
    $json = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8
    if ((Get-ReleaseFileIdentity $manifestPath) -cne $manifestIdentity) { throw 'manifest_changed' }
    $document = [System.Text.Json.JsonDocument]::Parse($json)
    try { Assert-UniqueJson $document.RootElement } finally { $document.Dispose() }
    $manifest = $json | ConvertFrom-Json -AsHashtable
    if ($manifest.version -cne '0.1.0' -or $manifest.target -cne 'x86_64-pc-windows-msvc' -or $manifest.signing -cne 'unsigned_candidate' -or $manifest.source_commit -cnotmatch '^[a-f0-9]{40}$' -or $manifest.source_digest -cnotmatch '^[a-f0-9]{64}$' -or $manifest.source_dirty -isnot [bool]) { throw 'invalid_metadata' }
    if ($manifest.checks.build -cne 'passed' -or $manifest.checks.package_static -cne 'passed') { throw 'invalid_checks' }
    foreach ($name in @('installation','upgrade','uninstall','real_agent','semantic_provider')) { if ($manifest.checks[$name] -cne 'not_run') { throw 'invalid_checks' } }
    $required = @('desktop/MindTrail.exe','producer/mindtrail-producer.exe','installer/MindTrail-setup.exe','nsis/installer.nsi')
    if ($manifest.files -isnot [array] -or $manifest.files.Count -ne $required.Count) { throw 'invalid_files' }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($item in $manifest.files) {
        $path = $item.path
        if ($path -isnot [string] -or $path -match '[\\:]' -or $path.StartsWith('/') -or $path.Split('/') -contains '..') { throw 'invalid_path' }
        if (-not $seen.Add($path)) { throw 'duplicate_path' }
        if (-not ($required -ccontains $path)) { throw 'invalid_path' }
        $parts = $path.Split('/')
        $parent = Get-Item -LiteralPath (Join-Path $rootPath $parts[0]) -Force
        if (-not $parent.PSIsContainer -or ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'invalid_path' }
        $file = Get-Item -LiteralPath (Join-Path $parent.FullName $parts[1]) -Force
        if ($file.PSIsContainer -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $item.size -isnot [long] -or $item.size -lt 1 -or $file.Length -ne $item.size -or $item.sha256 -cnotmatch '^[a-f0-9]{64}$') { throw 'invalid_file' }
        $identity=Get-ReleaseFileIdentity $file.FullName
        $digest = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        $after = Get-Item -LiteralPath $file.FullName -Force
        if ($digest -cne $item.sha256 -or $after.Length -ne $file.Length -or ($after.Attributes -band [IO.FileAttributes]::ReparsePoint) -or (Get-ReleaseFileIdentity $file.FullName) -cne $identity) { throw 'invalid_file' }
    }
    Write-Output 'release_integrity_verified'
    exit 0
} catch {
    if ($_.Exception.Message -ceq 'duplicate_path') { Write-Output 'release_duplicate_path' } else { Write-Output 'release_validation_failed' }
    exit 1
}
