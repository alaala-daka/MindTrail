param([string]$Library,[string]$Root)
. $Library
Get-ReleaseSourceFiles $Root | ConvertTo-Json -Compress
