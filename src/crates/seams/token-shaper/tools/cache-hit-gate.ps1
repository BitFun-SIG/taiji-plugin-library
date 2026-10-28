#requires -Version 5.1
<#
  cache-hit-gate.ps1

  Purpose : Regression gate for provider prefix-cache health.
            Reads the runtime's own usage telemetry and FAILS when the cache-hit
            ratio drops below a floor.

  Why     : Measured live (2026-09-17) the cache was hitting 88-99.9%. That
            headroom is the thing worth protecting, and nothing in the repo
            asserted it. Any change touching `tools` order, the `system` bytes,
            or an already-sent message can drop it to zero silently: the provider
            just bills more and reports nothing.

            The observable side already exists (`cache_read` is logged). The
            write side does NOT (`cache_write` is always None), so this gate
            deliberately asserts only on what is actually reported rather than
            inventing a signal.

  Usage   : powershell -NoProfile -File tools/cache-hit-gate.ps1
            powershell -NoProfile -File tools/cache-hit-gate.ps1 -Floor 0.50
            powershell -NoProfile -File tools/cache-hit-gate.ps1 -LogDir <path>

  Exit    : 0 = PASS (ratio >= floor)   1 = FAIL (ratio < floor)   2 = NO DATA
#>
[CmdletBinding()]
param(
    [double]$Floor = 0.50,
    [string]$LogDir = (Join-Path $env:APPDATA 'openbitfun\logs'),
    [int]$MaxFiles = 12
)

$ErrorActionPreference = 'Stop'

$logFiles = Get-ChildItem $LogDir -Recurse -File -Filter 'ai*.log' -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending |
            Select-Object -First $MaxFiles

if (-not $logFiles) {
    Write-Output "NO DATA: no ai*.log under $LogDir"
    exit 2
}

$samples = New-Object System.Collections.Generic.List[object]
foreach ($f in $logFiles) {
    foreach ($line in (Select-String -Path $f.FullName -Pattern 'OpenAI final token usage' -ErrorAction SilentlyContinue)) {
        $m = [regex]::Match($line.Line, 'input=(\d+).*?cache_read=Some\((\d+)\)')
        if ($m.Success) {
            $input = [double]$m.Groups[1].Value
            $read  = [double]$m.Groups[2].Value
            if ($input -gt 0) {
                $samples.Add([pscustomobject]@{
                    Ratio = $read / $input
                    Input = [int]$input
                    Read  = [int]$read
                    File  = $f.Name
                })
            }
        }
    }
}

if ($samples.Count -eq 0) {
    Write-Output "NO DATA: found ai*.log but no parsable 'OpenAI final token usage' lines"
    exit 2
}

# Weight by prompt size: a 200k-token request matters far more than a 50-token
# one, and an unweighted mean would let tiny requests mask a real regression.
$sumInput = ($samples | Measure-Object -Property Input -Sum).Sum
$sumRead  = ($samples | Measure-Object -Property Read  -Sum).Sum
$weighted = if ($sumInput -gt 0) { $sumRead / $sumInput } else { 0 }

$minRatio = ($samples | Measure-Object -Property Ratio -Minimum).Minimum

Write-Output '=== prefix-cache hit gate ==='
Write-Output ("samples      : {0} (from {1} log file(s))" -f $samples.Count, $logFiles.Count)
Write-Output ("weighted hit : {0:P2}  ({1} / {2} tokens)" -f $weighted, $sumRead, $sumInput)
Write-Output ("worst single : {0:P2}" -f $minRatio)
Write-Output ("floor        : {0:P2}" -f $Floor)
Write-Output ''

if ($weighted -lt $Floor) {
    Write-Output ("FAIL: weighted cache hit {0:P2} below floor {1:P2}" -f $weighted, $Floor)
    Write-Output '      Something is perturbing the cached prefix (tools order, system bytes,'
    Write-Output '      or an already-sent message). Check the most recent change touching'
    Write-Output '      the request body.'
    exit 1
}

Write-Output ("PASS: weighted cache hit {0:P2} >= floor {1:P2}" -f $weighted, $Floor)
exit 0
