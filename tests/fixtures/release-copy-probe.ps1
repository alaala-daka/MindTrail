param([string]$Library,[int]$BuildExitCode,[string]$BuiltPath,[string]$Destination)
$ErrorActionPreference='Stop'
try { . $Library; $null=Copy-ProducerAfterBuild $BuildExitCode $BuiltPath $Destination; exit 0 } catch { exit 1 }
