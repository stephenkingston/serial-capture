<#
.SYNOPSIS
    Send numbered lines to a serial port at full speed, for capture-loss tests.

.DESCRIPTION
    Every line starts with SEQ<number>, so a captured log can be checked for
    gaps with verify-capture.ps1. Each line is padded to a fixed payload size
    and terminated with CRLF, which makes the expected byte count exact:

        bytes = Lines * (PayloadBytes + 2)

    Writes go through the Windows serial driver, which blocks when the UART is
    busy. The script can therefore only under-report, never silently drop: if
    the capture sees fewer bytes, the loss happened after the host wrote them
    (USBPcap's ring buffer, or the USB-UART chip).

.EXAMPLE
    # Send 5000 x 50-byte lines to COM6 at 921600 baud as fast as the UART allows.
    .\tools\throughput-test.ps1 -Port COM6 -Baud 921600 -Lines 5000 -PayloadBytes 50

.EXAMPLE
    # Slow trickle: 200 lines, 20 ms apart, useful to bisect a loss threshold.
    .\tools\throughput-test.ps1 -Port COM6 -Lines 200 -DelayMs 20
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Port,

    [int]$Baud = 115200,

    [ValidateSet('None', 'Odd', 'Even', 'Mark', 'Space')]
    [string]$Parity = 'None',

    [int]$DataBits = 8,

    [ValidateSet('One', 'Two')]
    [string]$StopBits = 'One',

    # Total payload bytes per line, including "SEQ000000,".
    [int]$PayloadBytes = 50,

    [int]$Lines = 5000,

    # Sleep this long after each batch. 0 = as fast as the port accepts.
    [int]$DelayMs = 0,

    # Leave DTR/RTS alone. Some boards gate the UART on them.
    [switch]$NoHandshake
)

$ErrorActionPreference = 'Stop'

# "SEQ" + 6 digits + "," = 10 characters of overhead.
if ($PayloadBytes -lt 10) {
    throw "PayloadBytes must be at least 10 (SEQ000000,)."
}
if ($Lines -lt 1) {
    throw "Lines must be at least 1."
}

$pad = 'x' * ($PayloadBytes - 10)
$totalBytes = [int64]$Lines * ($PayloadBytes + 2)

$serial = New-Object System.IO.Ports.SerialPort
$serial.PortName = $Port
$serial.BaudRate = $Baud
$serial.Parity = [System.IO.Ports.Parity]$Parity
$serial.DataBits = $DataBits
$serial.StopBits = [System.IO.Ports.StopBits]$StopBits
$serial.Handshake = [System.IO.Ports.Handshake]::None
$serial.Encoding = [System.Text.Encoding]::ASCII
$serial.WriteTimeout = 30000
# Keep the driver-side queue small enough that a stuck receiver shows up as a
# timeout instead of a huge pile of buffered bytes.
$serial.WriteBufferSize = 65536

Write-Host "Opening $Port ($Baud $DataBits$($Parity.Substring(0,1))$StopBits) ..."
$serial.Open()
try {
    if (-not $NoHandshake) {
        $serial.DtrEnable = $true
        $serial.RtsEnable = $true
    }

    $batch = New-Object System.Text.StringBuilder 32768
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

    for ($i = 0; $i -lt $Lines; $i++) {
        [void]$batch.AppendFormat("SEQ{0:D6},{1}`r`n", $i, $pad)
        # ~8 KiB per Write keeps syscall overhead low without hiding the
        # sender's own progress for long.
        if ($batch.Length -ge 8192 -or $i -eq $Lines - 1) {
            $serial.Write($batch.ToString())
            [void]$batch.Clear()
            if ($DelayMs -gt 0) {
                Start-Sleep -Milliseconds $DelayMs
            }
        }
    }

    # SerialPort has no Flush(); the underlying stream does.
    $serial.BaseStream.Flush()
    $stopwatch.Stop()
}
finally {
    $serial.Close()
    $serial.Dispose()
}

$seconds = [Math]::Max($stopwatch.Elapsed.TotalSeconds, 0.001)
$rate = $totalBytes / $seconds

Write-Host ''
Write-Host ("Sent {0} line(s), {1} byte(s) in {2:N2} s ({3:N1} KiB/s)." -f `
        $Lines, $totalBytes, $seconds, ($rate / 1024))
Write-Host ("Expected capture: {0} line(s), {1} byte(s) on the wire." -f $Lines, $totalBytes)
Write-Host ("First line: SEQ{0:D6},...   Last line: SEQ{1:D6},..." -f 0, ($Lines - 1))
Write-Host ''
Write-Host 'Now run:  .\tools\verify-capture.ps1 -Path <capture.log>'
