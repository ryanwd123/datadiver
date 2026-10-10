param(
    [Parameter(Mandatory=$true)][string]$Binary,
    [ValidateSet('baseline', 'persistent')][string]$Backend = 'persistent',
    [int]$Repeats = 5
)
$ErrorActionPreference = 'Stop'
$taskRoot = (Resolve-Path 'target/performance').Path
$taskBinary = (Resolve-Path $Binary).Path
foreach ($fixture in @('10k', '100k', '1m', 'wide')) {
    foreach ($repeat in 1..$Repeats) {
        $taskProfile = Join-Path $taskRoot ("ui-profiles/$Backend-$fixture-$repeat-" + [guid]::NewGuid().ToString('N'))
        $phases = if ($Backend -eq 'persistent') { @('import', 'reopen') } else { @('import') }
        foreach ($phase in $phases) {
            $taskLog = Join-Path $taskRoot "$Backend-ui-$fixture-$repeat-$phase.json"
            $taskSource = Join-Path $taskRoot "fixtures/$fixture.csv"
            $taskArgs = @('--file', ('"' + $taskSource + '"'), '--window', '900x600', '--no-restore', '--benchmark-ui', ('"' + $taskLog + '"'))
            if ($Backend -eq 'baseline') { $taskArgs += '--ephemeral' }
            else { $taskArgs += @('--home', ('"' + $taskProfile + '"')) }
            # Actual GUI windows must be visible for Windows to deliver drawing frames.
            $taskProcess = Start-Process -FilePath $taskBinary -ArgumentList $taskArgs -WindowStyle Normal -PassThru
            $taskTimer = [System.Diagnostics.Stopwatch]::StartNew()
            $taskSamples = [System.Collections.Generic.List[object]]::new()
            while (-not $taskProcess.HasExited) {
                $taskProcess.Refresh()
                $taskSamples.Add(@{ ms=$taskTimer.Elapsed.TotalMilliseconds; working_set=$taskProcess.WorkingSet64; private_bytes=$taskProcess.PrivateMemorySize64; peak_working_set=$taskProcess.PeakWorkingSet64 })
                if ($taskTimer.Elapsed.TotalSeconds -gt 90) {
                    Stop-Process -Id $taskProcess.Id
                    throw "UI benchmark timed out: $Backend $fixture $phase"
                }
                Start-Sleep -Milliseconds 50
            }
            $taskProcess.WaitForExit()
            if ($taskProcess.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $taskLog)) { throw "UI benchmark failed: $Backend $fixture $phase" }
            $taskEvents = Get-Content -LiteralPath $taskLog -Raw | ConvertFrom-Json
            if (@($taskEvents | Where-Object metric -eq 'first_useful_frame').Count -ne 1 -or
                @($taskEvents | Where-Object metric -eq 'search_to_frame').Count -ne 3 -or
                @($taskEvents | Where-Object metric -eq 'scroll_frame_interval').Count -ne 120) { throw "Incomplete UI measurements: $Backend $fixture $phase" }
            $taskSamples | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath ($taskLog + '.memory.json')
        }
    }
}
