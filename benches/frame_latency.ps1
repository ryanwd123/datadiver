<#
.SYNOPSIS
Measures DataDiver's frame times and input-to-screen latency while scrolling.

.DESCRIPTION
Starts the app once per scenario, variant and run, lets it settle, then runs
benches/frame_probe: it posts mouse-wheel or Down-arrow input to the window at
a fixed rate for a few seconds and reads each frame's number off the screen
with Desktop Duplication. Writes one CSV row per run and prints the medians.

The app must be built with performance/frame-experiments.patch applied: its
DD_FRAME_TRACE switch draws the frame marker and writes the per-frame trace,
and DD_PRESENT / DD_FLIP pick the present path. Without the patch the probe
finds no trace. See PERFORMANCE.md, 2026-11-21.

The window is made topmost while it is measured; don't use the mouse wheel or
keyboard over it, since stray input skews the match between posts and frames.

.EXAMPLE
git apply performance/frame-experiments.patch
cargo build --profile perf
cargo build --release --manifest-path benches/frame_probe/Cargo.toml
.\benches\frame_latency.ps1 -Runs 3
.\benches\frame_latency.ps1 -Only wheel-120 -Variants baseline,flip-wait
#>
param(
    [int]$Runs = 3,
    [double]$Seconds = 5,
    [int]$Settle = 6,
    [string]$Exe = "$PSScriptRoot\..\target\perf\datadiver.exe",
    [string]$Probe = "$PSScriptRoot\frame_probe\target\release\frame_probe.exe",
    [string]$Output = "$PSScriptRoot\..\target\frame_latency.csv",
    [string[]]$Only = @(),
    [string[]]$Variants = @('baseline', 'immediately', 'flip', 'flip-0', 'flip-wait', 'flip-loop', 'flip-loop-0')
)

$variantEnv = @{
    'baseline'    = @{}
    'immediately' = @{ DD_PRESENT = 'immediately' }
    'flip'        = @{ DD_FLIP = '1' }
    'flip-wait'   = @{ DD_FLIP = 'wait' }
    'flip-0'      = @{ DD_FLIP = '0' }
    'flip-loop'   = @{ DD_FLIP = 'loop' }
    'flip-loop-0' = @{ DD_FLIP = 'loop0' }
}
$common = @('--ephemeral', '--no-restore', '--rows', '1000000')
$scenarios = [ordered]@{
    # A fast wheel spin or a precision touchpad: more input than frames.
    'wheel-120'       = @{ Input = 'wheel'; Rate = 120; Window = '1200x800' }
    'wheel-120-large' = @{ Input = 'wheel'; Rate = 120; Window = '3600x1400' }
    # A held arrow key at the default keyboard repeat rate.
    'key-30'          = @{ Input = 'key'; Rate = 30; Window = '1200x800' }
}

$trace = Join-Path (Split-Path $Output) 'frame_trace.txt'
$rows = @()
foreach ($name in $scenarios.Keys) {
    if ($Only.Count -and $name -notin $Only) { continue }
    $scenario = $scenarios[$name]
    foreach ($variant in $Variants) {
        for ($run = 1; $run -le $Runs; $run++) {
            Remove-Item $trace -ErrorAction SilentlyContinue
            $env:DD_FRAME_TRACE = $trace
            foreach ($key in $variantEnv[$variant].Keys) { Set-Item "env:$key" $variantEnv[$variant][$key] }
            $process = Start-Process -FilePath $Exe -ArgumentList ($common + @('--window', $scenario.Window)) -PassThru
            foreach ($key in @('DD_FRAME_TRACE', 'DD_PRESENT', 'DD_FLIP')) { Remove-Item "env:$key" -ErrorAction SilentlyContinue }
            Start-Sleep -Seconds $Settle
            $json = & $Probe --pid $process.Id --trace $trace --input $scenario.Input --rate $scenario.Rate --seconds $Seconds --label $variant
            if (-not $process.WaitForExit(10000)) { Stop-Process -Id $process.Id -Force }
            if (-not $json) { Write-Warning "$name $variant #$run produced no result"; continue }
            $r = $json | ConvertFrom-Json
            $row = [pscustomobject]@{
                Scenario     = $name
                Variant      = $variant
                Run          = $run
                Matched      = "$($r.matched)/$($r.posts)"
                Dispatched   = $r.dispatched
                BadMarkers   = $r.bad_markers
                FramesPerSec = $r.frames_per_s
                ShownPerSec  = $r.shown_per_s
                MCyclesPerSec = $r.mcycles_per_s
                ScreenP50    = $r.to_screen.p50
                ScreenP95    = $r.to_screen.p95
                ScreenP99    = $r.to_screen.p99
                SubmitP50    = $r.to_submit.p50
                DispatchP50  = $r.to_dispatch.p50
                DispatchP95  = $r.to_dispatch.p95
                PaintP50     = $r.paint.p50
                PaintP95     = $r.paint.p95
                EndDrawP50   = $r.end_draw.p50
                PresentP50   = $r.present.p50
                WaitP50      = $r.wait.p50
                IntervalP50  = $r.interval.p50
                IntervalP95  = $r.interval.p95
            }
            $rows += $row
            Write-Host ("{0} {1} #{2}: screen p50 {3} p95 {4} ms, {5} fps ({6} shown), paint {7} ms, EndDraw {8} ms, {9} Mcycles/s, matched {10}" -f `
                $name, $variant, $run, $row.ScreenP50, $row.ScreenP95, $row.FramesPerSec, $row.ShownPerSec, $row.PaintP50, $row.EndDrawP50, $row.MCyclesPerSec, $row.Matched)
            Start-Sleep -Milliseconds 500
        }
    }
}
$rows | Export-Csv -NoTypeInformation -Path $Output
"`nMedians of the per-run values (written $Output):"
$median = { param($values) $s = @($values | Sort-Object); $s[[int](($s.Count - 1) / 2)] }
$range = { param($values) "$(($values | Measure-Object -Minimum).Minimum)-$(($values | Measure-Object -Maximum).Maximum)" }
$rows | Group-Object Scenario, Variant | ForEach-Object {
    $g = $_.Group
    [pscustomobject]@{
        Scenario   = $g[0].Scenario
        Variant    = $g[0].Variant
        ScreenP50  = & $median $g.ScreenP50
        P50Range   = & $range $g.ScreenP50
        ScreenP95  = & $median $g.ScreenP95
        SubmitP50  = & $median $g.SubmitP50
        Fps        = & $median $g.FramesPerSec
        Shown      = & $median $g.ShownPerSec
        PaintP50   = & $median $g.PaintP50
        PaintP95   = & $median $g.PaintP95
        EndDrawP50 = & $median $g.EndDrawP50
        PresentP50 = & $median $g.PresentP50
        WaitP50    = & $median $g.WaitP50
        MCycles    = & $median $g.MCyclesPerSec
    }
} | Format-Table -AutoSize
