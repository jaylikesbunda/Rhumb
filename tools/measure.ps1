# Measures Xplor against explorer.exe on this machine.
#
# Everything here is automatable except the folder-open timing, which Xplor's
# own status bar reports as it happens. See the note printed at the end.
#
# Usage, from a fresh login with both apps closed:
#   pwsh tools/measure.ps1
#   pwsh tools/measure.ps1 -RestartExplorer   # kills explorer to time a cold start
#
# Cold start is only meaningful straight after a login. A warm start from
# Start-Process flatters both processes, so the README quotes the after-login
# figure and this script's figure is labelled accordingly.

[CmdletBinding()]
param(
    [string]$XplorPath,
    [int]$IdleSeconds = 30,
    [switch]$RestartExplorer
)

$ErrorActionPreference = 'Stop'

# Resolved here rather than in the param default: $PSScriptRoot is not yet bound
# when defaults are evaluated under `powershell -File`.
if (-not $XplorPath) {
    $root = Split-Path (Split-Path $PSCommandPath -Parent) -Parent
    $XplorPath = Join-Path $root 'target\release\xplor.exe'
}

function Get-IdleCpu {
    param([string]$Name, [int]$Seconds)
    $p = Get-Process -Name $Name -ErrorAction SilentlyContinue
    if (-not $p) { return $null }
    $before = $p.TotalProcessorTime
    $start = Get-Date
    Start-Sleep -Seconds $Seconds
    $p.Refresh()
    $spent = ($p.TotalProcessorTime - $before).TotalSeconds
    $wall = ((Get-Date) - $start).TotalSeconds
    # Mean percent of a single core over the window.
    [math]::Round(($spent / $wall) * 100, 2)
}

function Get-MemMb {
    param([string]$Name)
    $p = Get-Process -Name $Name -ErrorAction SilentlyContinue
    if (-not $p) { return $null }
    [math]::Round((($p | Measure-Object -Property WorkingSet64 -Sum).Sum) / 1MB, 0)
}

function Measure-ColdStart {
    param([string]$Exe)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $Exe -PassThru
    while ($sw.ElapsedMilliseconds -lt 20000) {
        $p.Refresh()
        if ($p.MainWindowHandle -ne 0) { break }
        Start-Sleep -Milliseconds 10
    }
    $sw.Stop()
    $p.CloseMainWindow() | Out-Null
    $sw.ElapsedMilliseconds
}

if (-not (Test-Path $XplorPath)) {
    throw "xplor.exe not found at $XplorPath. Run 'cargo build --release' first."
}

$machine = Get-CimInstance Win32_ComputerSystem
$cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name

'=== environment ==='
"CPU        : $cpu"
"Logical    : $($machine.NumberOfLogicalProcessors) threads"
"RAM        : $([math]::Round($machine.TotalPhysicalMemory / 1GB, 1)) GB"

''
'=== cold start to first window (WARM - see header) ==='
if ($RestartExplorer) {
    'Restarting explorer.exe, this will close any open Explorer windows...'
    Stop-Process -Name explorer -Force
    Start-Sleep -Seconds 3
    "explorer.exe : $(Measure-ColdStart explorer.exe) ms"
} else {
    'explorer.exe : skipped (pass -RestartExplorer to measure; this closes Explorer windows)'
}
"xplor.exe    : $(Measure-ColdStart $XplorPath) ms"

''
"=== idle over $IdleSeconds s, one folder open ==="
'process     idle CPU    working set'
foreach ($name in 'explorer', 'xplor') {
    $cpuPct = Get-IdleCpu -Name $name -Seconds $IdleSeconds
    $mem = Get-MemMb -Name $name
    if ($null -eq $cpuPct) {
        "{0,-12} (not running)" -f $name
    } else {
        "{0,-12} {1,6}% {2,10} MB" -f $name, $cpuPct, $mem
    }
}

''
'=== on disk ==='
$xplorSize = [math]::Round((Get-Item $XplorPath).Length / 1MB, 1)
$xploreSize = [math]::Round((Get-Item "$env:SystemRoot\explorer.exe").Length / 1MB, 1)
"xplor.exe    $xplorSize MB"
"explorer.exe $xploreSize MB (system component)"

''
'=== folder open time ==='
'Not automatable here. Open a folder with a known file count, read the'
'figure from the status bar at the bottom right of the window, repeat five'
'times and take the median. That number is reproducible by the reader, which'
'is why it is the one worth putting in the README.'
