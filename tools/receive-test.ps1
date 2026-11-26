<#
.SYNOPSIS
    Read a serial port for a fixed time and save the bytes, for loss tests.

.DESCRIPTION
    Opens the port (which also programs the USB-UART chip's baud rate, needed
    when only a passive capture runs on that port) and drains it continuously.
    Prints how many bytes arrived and saves them to a text file that
    verify-capture.ps1 can scan for SEQ gaps.

    Compare three numbers after a run:

      sender bytes      == receiver bytes?   no -> UART / baud / chip loss
      receiver bytes    == capture bytes?    no -> USBPcap / capture exe loss

.EXAMPLE
    .\tools\receive-test.ps1 -Port COM6 -Baud 921600 -Seconds 30 -OutFile .\rx.log
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Port,

    [int]$Baud = 921600,

    # Stop after this many seconds so the script always closes the port and
    # flushes the file, even when no one presses Ctrl-C.
    [int]$Seconds = 30,

    [string]$OutFile = "rx.log",

    [ValidateSet('None', 'Odd', 'Even', 'Mark', 'Space')]
    [string]$Parity = 'None',

    [int]$DataBits = 8,

    [ValidateSet('One', 'Two')]
    [string]$StopBits = 'One'
)

$ErrorActionPreference = 'Stop'

$serial = New-Object System.IO.Ports.SerialPort
$serial.PortName = $Port
$serial.BaudRate = $Baud
$serial.Parity = [System.IO.Ports.Parity]$Parity
$serial.DataBits = $DataBits
$serial.StopBits = [System.IO.Ports.StopBits]$StopBits
$serial.Handshake = [System.IO.Ports.Handshake]::None
$serial.Encoding = [System.Text.Encoding]::ASCII
$serial.ReadBufferSize = 65536

$writer = New-Object System.IO.StreamWriter($OutFile, $false, [System.Text.Encoding]::ASCII)
$total = 0
$stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

Write-Host "Reading $Port at $Baud baud for $Seconds s -> $OutFile ..."
$serial.Open()
try {
    # DTR/RTS asserted the same way the sender does; harmless on most adapters.
    $serial.DtrEnable = $true
    $serial.RtsEnable = $true

    while ($stopwatch.Elapsed.TotalSeconds -lt $Seconds) {
        $chunk = $serial.ReadExisting()
        if ($chunk.Length -eq 0) {
            Start-Sleep -Milliseconds 5
            continue
        }
        $writer.Write($chunk)
        $total += $chunk.Length
    }
}
finally {
    $serial.Close()
    $serial.Dispose()
    $writer.Flush()
    $writer.Dispose()
    $stopwatch.Stop()
}

$rate = $total / [Math]::Max($stopwatch.Elapsed.TotalSeconds, 0.001)
Write-Host ''
Write-Host ("Received {0} byte(s) in {1:N2} s ({2:N1} KiB/s)." -f `
        $total, $stopwatch.Elapsed.TotalSeconds, ($rate / 1024))
Write-Host ("Saved to {0}" -f (Resolve-Path -LiteralPath $OutFile))
Write-Host ''
Write-Host ("Now run:  .\tools\verify-capture.ps1 -Path {0} -Expected <lines>" -f $OutFile)
