<#
.SYNOPSIS
    Check a serial-capture log for missing SEQ numbers.

.DESCRIPTION
    Scans the capture text for tokens like SEQ000123 and reports gaps. A gap
    means bytes were lost somewhere between the sender and the log - USBPcap's
    driver ring buffer, the USB-UART chip, or the capture process.

    Use ASCII or Both format (the default). With Both, the token appears in the
    ASCII column. Hex-only logs cannot be checked this way.

.EXAMPLE
    .\tools\verify-capture.ps1 -Path .\out.log

.EXAMPLE
    # The sender used -Lines 5000; require every number in 0..4999.
    .\tools\verify-capture.ps1 -Path .\out.log -Expected 5000
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Path,

    # Regex with one capture group holding the decimal sequence number.
    [string]$Pattern = 'SEQ(\d{6})',

    # Expected count; the check covers 0..Expected-1. 0 = infer from the
    # highest number seen (still catches gaps, but not a missing tail).
    [int]$Expected = 0,

    # How many missing ranges to print.
    [int]$MaxRanges = 20
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Path)) {
    throw "No such file: $Path"
}

$full = (Resolve-Path -LiteralPath $Path).ProviderPath
$regex = [regex]::new($Pattern)
$seen = [System.Collections.Generic.HashSet[int]]::new()
$tokenCount = 0
$parseErrors = 0
$min = [int]::MaxValue
$max = [int]::MinValue

$reader = [System.IO.StreamReader]::new($full)
try {
    while ($null -ne ($line = $reader.ReadLine())) {
        foreach ($m in $regex.Matches($line)) {
            $tokenCount++
            $n = 0
            if ([int]::TryParse($m.Groups[1].Value, [ref]$n)) {
                [void]$seen.Add($n)
                if ($n -lt $min) { $min = $n }
                if ($n -gt $max) { $max = $n }
            }
            else {
                $parseErrors++
            }
        }
    }
}
finally {
    $reader.Dispose()
}

if ($seen.Count -eq 0) {
    Write-Host "No '$Pattern' tokens found in $Path." -ForegroundColor Red
    exit 2
}

$lower = if ($Expected -gt 0) { 0 } else { $min }
$upper = if ($Expected -gt 0) { $Expected - 1 } else { $max }

# Walk the expected range once: count gaps and compress them into ranges.
$ranges = [System.Collections.Generic.List[string]]::new()
$missing = 0
$start = -1
$prev = -1
for ($n = $lower; $n -le $upper; $n++) {
    if (-not $seen.Contains($n)) {
        if ($start -lt 0) { $start = $n }
        $prev = $n
        $missing++
    }
    elseif ($start -ge 0) {
        $ranges.Add($(if ($start -eq $prev) { "$start" } else { "$start-$prev" }))
        $start = -1
    }
}
if ($start -ge 0) {
    $ranges.Add($(if ($start -eq $prev) { "$start" } else { "$start-$prev" }))
}

Write-Host ("File:              {0}" -f $full)
Write-Host ("Tokens found:      {0} ({1} distinct)" -f $tokenCount, $seen.Count)
Write-Host ("Observed range:    {0}..{1}" -f $min, $max)
Write-Host ("Expected range:    {0}..{1} ({2} line(s))" -f $lower, $upper, ($upper - $lower + 1))

if ($parseErrors -gt 0) {
    Write-Host ("Unparsable tokens: {0}" -f $parseErrors) -ForegroundColor Yellow
}

if ($missing -le 0) {
    Write-Host 'Result:            OK - no gaps.' -ForegroundColor Green
    exit 0
}

Write-Host ("Result:            {0} line(s) MISSING" -f $missing) -ForegroundColor Red
Write-Host ("Missing ranges:    {0}" -f $ranges.Count)
$shown = [Math]::Min($ranges.Count, $MaxRanges)
for ($i = 0; $i -lt $shown; $i++) {
    Write-Host ("  missing {0}" -f $ranges[$i])
}
if ($ranges.Count -gt $shown) {
    Write-Host ("  ... and {0} more range(s)" -f ($ranges.Count - $shown))
}

exit 1
