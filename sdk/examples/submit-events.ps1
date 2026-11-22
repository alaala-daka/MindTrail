param(
    [Parameter(Mandatory = $true)][string]$DiscoveryPath,
    [Parameter(Mandatory = $true)][string]$BatchPath,
    [Parameter(Mandatory = $true)][string]$ScratchDirectory
)
$ErrorActionPreference = 'Stop'

# Use the exact discovery file and batch supplied by the caller. Do not print either
# discovery content or the authorization configuration.
function Test-UniqueJson([System.Text.Json.JsonElement]$Element) {
    if ($Element.ValueKind -eq [System.Text.Json.JsonValueKind]::Object) {
        $seen = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
        foreach ($property in $Element.EnumerateObject()) {
            if (-not $seen.Add($property.Name) -or -not (Test-UniqueJson $property.Value)) { return $false }
        }
    } elseif ($Element.ValueKind -eq [System.Text.Json.JsonValueKind]::Array) {
        foreach ($item in $Element.EnumerateArray()) { if (-not (Test-UniqueJson $item)) { return $false } }
    }
    return $true
}
function Read-LimitedObject([string]$Path, [int]$Limit) {
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $bytes = [byte[]]::new($Limit + 1); $count = 0
        while ($count -lt $bytes.Length) { $read = $stream.Read($bytes, $count, $bytes.Length - $count); if ($read -eq 0) { break }; $count += $read }
        if ($count -gt $Limit) { throw 'Input exceeds limit.' }
        $text = [System.Text.UTF8Encoding]::new($false, $true).GetString($bytes, 0, $count)
    } finally { $stream.Dispose() }
    $document = [System.Text.Json.JsonDocument]::Parse([string]$text)
    try {
        if ($document.RootElement.ValueKind -ne [System.Text.Json.JsonValueKind]::Object -or -not (Test-UniqueJson $document.RootElement)) { throw 'Invalid JSON object.' }
    } finally { $document.Dispose() }
    return ConvertFrom-Json -InputObject $text -AsHashtable
}
try { $discovery = Read-LimitedObject -Path $DiscoveryPath -Limit 4096 }
catch { throw 'Invalid gateway discovery.' }
if ($discovery -isnot [System.Collections.IDictionary] -or $discovery.Count -ne 4 -or
    ($discovery.version -isnot [int] -and $discovery.version -isnot [long]) -or $discovery.version -ne 1 -or
    ($discovery.port -isnot [int] -and $discovery.port -isnot [long]) -or $discovery.port -lt 1 -or $discovery.port -gt 65535 -or
    $discovery.token -isnot [string] -or $discovery.store_id -isnot [string] -or
    $discovery.token -cnotmatch '^[0-9a-f]{64}$' -or $discovery.store_id -cnotmatch '^[0-9a-f]{64}$') {
    throw 'Invalid gateway discovery.'
}
$batchFile = (Resolve-Path -LiteralPath $BatchPath).Path
if ((Get-Item -LiteralPath $batchFile).Length -gt 1MB) { throw 'Batch exceeds the protocol byte limit.' }
try { $batch = ConvertFrom-Json -InputObject (Get-Content -LiteralPath $batchFile -Raw -Encoding utf8) -AsHashtable -NoEnumerate }
catch { throw 'Invalid event batch.' }
if ($batch -isnot [array] -or $batch.Count -lt 1 -or $batch.Count -gt 200) { throw 'Batch must contain 1–200 events.' }
$lastRequested = 0
$session = $null
foreach ($event in $batch) {
    if ($event -isnot [System.Collections.IDictionary] -or $event.session -isnot [string] -or [string]::IsNullOrWhiteSpace($event.session)) { throw 'Invalid event session.' }
    if ($null -eq $session) { $session = $event.session } elseif ($session -cne $event.session) { throw 'Batch must contain one session.' }
    if ($event.seq -isnot [long] -and $event.seq -isnot [int]) { throw 'Invalid event sequence.' }
    if ($event.seq -le $lastRequested -or $event.seq -gt 9007199254740991) { throw 'Invalid event sequence.' }
    $lastRequested = $event.seq
}
$scratch = (Resolve-Path -LiteralPath $ScratchDirectory).Path
$outputPath = Join-Path $scratch ([System.IO.Path]::GetRandomFileName())
$file = [System.IO.File]::Open($outputPath, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
$file.Dispose()
$authConfig = 'header = "Authorization: Bearer ' + $discovery.token + '"'
$base = 'http://127.0.0.1:' + $discovery.port
function Invoke-Gateway([string[]]$RequestArguments) {
    $httpStatus = $authConfig | & curl.exe -q --silent --show-error --noproxy '*' --connect-timeout 2 --max-time 5 --max-redirs 0 --max-filesize 65536 --config - --output $outputPath --write-out '%{http_code}' @RequestArguments
    if ($LASTEXITCODE -ne 0 -or $httpStatus -ne '200') { throw 'Gateway request failed.' }
    try { return Read-LimitedObject -Path $outputPath -Limit 65536 }
    catch { throw 'Invalid gateway response.' }
}
try {
    $curlVersion = & curl.exe -q --version | Select-Object -First 1
    if ($curlVersion -notmatch '^curl (\d+)\.(\d+)\.' -or [int]$Matches[1] -lt 8 -or ([int]$Matches[1] -eq 8 -and [int]$Matches[2] -lt 4)) { throw 'curl 8.4 or later is required.' }
    $health = Invoke-Gateway -RequestArguments @($base + '/v1/health')
    if ($health -isnot [System.Collections.IDictionary] -or $health.Count -ne 2 -or ($health.version -isnot [int] -and $health.version -isnot [long]) -or $health.version -ne 1 -or $health.store_id -isnot [string] -or $health.store_id -cne $discovery.store_id) { throw 'Gateway identity mismatch.' }
    $receipt = Invoke-Gateway -RequestArguments @('--request', 'POST', '--header', 'Content-Type: application/json', '--data-binary', ('@' + $batchFile), ($base + '/v1/events'))
    if ($receipt -isnot [System.Collections.IDictionary] -or $receipt.Count -ne 3 -or $receipt.accepted -isnot [long] -and $receipt.accepted -isnot [int] -or
        $receipt.duplicates -isnot [long] -and $receipt.duplicates -isnot [int] -or
        $receipt.last_seq -isnot [long] -and $receipt.last_seq -isnot [int] -or
        $receipt.accepted -lt 0 -or $receipt.duplicates -lt 0 -or
        $receipt.accepted + $receipt.duplicates -ne $batch.Count -or $receipt.last_seq -lt $lastRequested -or
        $receipt.last_seq -gt 9007199254740991) { throw 'Invalid gateway receipt.' }
    $receipt | ConvertTo-Json -Compress
} finally {
    Remove-Item -LiteralPath $outputPath -Force -ErrorAction SilentlyContinue
}
