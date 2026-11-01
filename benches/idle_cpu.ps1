<#
.SYNOPSIS
Measures DataDiver's idle CPU time and context switches.

.DESCRIPTION
Starts the release binary once per scenario and run, waits for it to settle,
then records the CPU cycles and per-thread context switches the process uses
over an idle window, with the switches grouped by the module each thread
started in. Nothing is clicked or typed; the window is only minimized for the
"minimized" states. Writes one CSV row per run and prints the medians.

The DD_* switches (DD_TRACE_PAINT for the paint count, DD_SOFTWARE,
DD_NO_BLINK, DD_NO_MONITOR, ...) exist only on the local
perf/idle-experiments branch; on other builds they do nothing and the paint
count reads 0. See PERFORMANCE.md, 2026-10-31.

.EXAMPLE
cargo build --release
.\benches\idle_cpu.ps1 -Runs 3 -Seconds 30
.\benches\idle_cpu.ps1 -Only table -Env @{ DD_SOFTWARE = '1' } -Label software
#>
param(
    [int]$Runs = 3,
    [int]$Seconds = 30,
    [int]$Settle = 8,
    [string]$Exe = "$PSScriptRoot\..\target\release\datadiver.exe",
    [string]$Output = "$PSScriptRoot\..\target\idle_cpu.csv",
    [string[]]$Only = @(),
    # Environment variables for the app, e.g. @{ DD_SOFTWARE = '1' }.
    [hashtable]$Env = @{},
    [string]$Label = 'baseline'
)

foreach ($key in $Env.Keys) { Set-Item "env:$key" $Env[$key] }

Add-Type -Namespace Idle -Name Win -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr hwnd, int cmd);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hwnd, out uint pid);
[DllImport("ntdll.dll")] public static extern int NtQueryInformationThread(System.IntPtr thread, int infoClass, out System.IntPtr info, int length, System.IntPtr returned);
[DllImport("kernel32.dll")] public static extern System.IntPtr OpenThread(int access, bool inherit, int id);
[DllImport("kernel32.dll")] public static extern bool CloseHandle(System.IntPtr handle);
[DllImport("kernel32.dll")] public static extern bool QueryProcessCycleTime(System.IntPtr process, out ulong cycles);
'@

# A file source, so the source monitor has a file to check.
$fixture = Join-Path (Split-Path $Output) 'idle_fixture.csv'
if (-not (Test-Path $fixture)) {
    $lines = [Collections.Generic.List[string]]::new()
    $lines.Add('id,region,amount,label,day')
    $regions = 'north', 'south', 'east', 'west', 'central', 'coast', 'hills', 'plains'
    for ($i = 0; $i -lt 100000; $i++) {
        $lines.Add("$i,$($regions[$i % 8]),$(($i * 37) % 10000 / 100),item $($i % 997),2026-$('{0:d2}' -f ($i % 12 + 1))-$('{0:d2}' -f ($i % 28 + 1))")
    }
    [IO.File]::WriteAllLines($fixture, $lines)
}

$common = @('--ephemeral', '--no-restore', '--window', '1200x800')
$scenarios = [ordered]@{
    'table'         = @{ Args = @('--rows', '1000000'); State = 'shown' }
    'csv-file'      = @{ Args = @('--file', $fixture); State = 'shown' }
    'table-min'     = @{ Args = @('--rows', '1000000'); State = 'minimized' }
    'sql-editor'    = @{ Args = @('--rows', '1000000', '--show', 'editor:sql'); State = 'shown' }
    'sql-editor-min'= @{ Args = @('--rows', '1000000', '--show', 'editor:sql'); State = 'minimized' }
    'database-tree' = @{ Args = @('--rows', '1000000', '--show', 'database-tree'); State = 'shown' }
}

function Switches([int]$processId) {
    # Raw per-thread counters are cumulative counts; the "Persec" name is the
    # formatted counter's.
    $threads = Get-CimInstance Win32_PerfRawData_PerfProc_Thread -Filter "IDProcess = $processId"
    $map = @{}
    foreach ($t in $threads) { $map[[int]$t.IDThread] = [int64]$t.ContextSwitchesPersec }
    $map
}

# The module each thread started in (its Win32 start address), so wakeups can
# be told apart: datadiver.exe, the C runtime (Rust threads), or a GPU driver.
function Modules($process) {
    $process.Refresh()
    $modules = $process.Modules | ForEach-Object {
        [pscustomobject]@{ Name = $_.ModuleName; Base = [int64]$_.BaseAddress; End = [int64]$_.BaseAddress + $_.ModuleMemorySize }
    }
    $map = @{}
    foreach ($thread in $process.Threads) {
        $handle = [Idle.Win]::OpenThread(0x40, $false, $thread.Id)  # THREAD_QUERY_INFORMATION
        $start = [IntPtr]::Zero
        [void][Idle.Win]::NtQueryInformationThread($handle, 9, [ref]$start, 8, [IntPtr]::Zero)  # ThreadQuerySetWin32StartAddress
        [void][Idle.Win]::CloseHandle($handle)
        $address = [int64]$start
        $module = $modules | Where-Object { $address -ge $_.Base -and $address -lt $_.End } | Select-Object -First 1
        $map[$thread.Id] = if ($module) { $module.Name } else { '?' }
    }
    $map
}

$rows = @()
foreach ($name in $scenarios.Keys) {
    if ($Only.Count -and $name -notin $Only) { continue }
    $scenario = $scenarios[$name]
    for ($run = 1; $run -le $Runs; $run++) {
        # The scratch build's DD_TRACE_PAINT writes one stderr line per paint.
        $trace = Join-Path (Split-Path $Output) 'idle_paints.txt'
        $process = Start-Process -FilePath $Exe -ArgumentList ($common + $scenario.Args) -PassThru -RedirectStandardError $trace
        Start-Sleep -Seconds $Settle
        $process.Refresh()
        if ($scenario.State -eq 'minimized') {
            [Idle.Win]::ShowWindow($process.MainWindowHandle, 6) | Out-Null  # SW_MINIMIZE
            Start-Sleep -Seconds 2
        }
        $foregroundPid = 0
        [Idle.Win]::GetWindowThreadProcessId([Idle.Win]::GetForegroundWindow(), [ref]$foregroundPid) | Out-Null
        $before = Switches $process.Id
        $paintsBefore = @(Get-Content $trace | Where-Object { $_ -like 'paint *' }).Count
        $process.Refresh()
        $cpuBefore = $process.TotalProcessorTime
        # Tick-sampled CPU time misses short bursts; cycle counts are exact.
        $cyclesBefore = [uint64]0
        [void][Idle.Win]::QueryProcessCycleTime($process.Handle, [ref]$cyclesBefore)
        $clock = [Diagnostics.Stopwatch]::StartNew()
        Start-Sleep -Seconds $Seconds
        $process.Refresh()
        $cpuAfter = $process.TotalProcessorTime
        $cyclesAfter = [uint64]0
        [void][Idle.Win]::QueryProcessCycleTime($process.Handle, [ref]$cyclesAfter)
        $after = Switches $process.Id
        $paintsAfter = @(Get-Content $trace | Where-Object { $_ -like 'paint *' }).Count
        $owners = Modules $process
        $elapsed = $clock.Elapsed.TotalSeconds
        $deltas = foreach ($id in $after.Keys) {
            [pscustomobject]@{ Thread = $id; Switches = $after[$id] - ($before[$id] ?? 0) }
        }
        $total = ($deltas | Measure-Object Switches -Sum).Sum
        $busiest = $deltas | Sort-Object Switches -Descending | Select-Object -First 3
        $byModule = $deltas | Group-Object { $owners[$_.Thread] ?? '?' } | ForEach-Object {
            [pscustomobject]@{ Module = $_.Name; PerSec = ($_.Group | Measure-Object Switches -Sum).Sum / $elapsed }
        } | Where-Object PerSec -ge 0.05 | Sort-Object PerSec -Descending
        $row = [pscustomobject]@{
            Label          = $Label
            Scenario       = $name
            Run            = $run
            Foreground     = ($foregroundPid -eq $process.Id)
            Seconds        = [math]::Round($elapsed, 2)
            CpuMsPerSec    = [math]::Round(($cpuAfter - $cpuBefore).TotalMilliseconds / $elapsed, 3)
            MCyclesPerSec  = [math]::Round(($cyclesAfter - $cyclesBefore) / 1e6 / $elapsed, 2)
            SwitchesPerSec = [math]::Round($total / $elapsed, 2)
            Threads        = $after.Count
            PaintsPerSec   = [math]::Round(($paintsAfter - $paintsBefore) / $elapsed, 2)
            ByModule       = ($byModule | ForEach-Object { "$($_.Module):$([math]::Round($_.PerSec, 1))" }) -join ' '
            Busiest        = ($busiest | ForEach-Object { "$($_.Thread):$([math]::Round($_.Switches / $elapsed, 1))" }) -join ' '
            WorkingSetMB   = [math]::Round($process.WorkingSet64 / 1MB, 1)
        }
        $rows += $row
        Write-Host "$Label $name #$run  cpu $($row.MCyclesPerSec) Mcycles/s  switches $($row.SwitchesPerSec)/s  paints $($row.PaintsPerSec)/s  [$($row.ByModule)]  fg=$($row.Foreground)"
        Stop-Process -Id $process.Id -Force
        Start-Sleep -Seconds 1
    }
}
foreach ($key in $Env.Keys) { Remove-Item "env:$key" -ErrorAction SilentlyContinue }
$rows | Export-Csv -NoTypeInformation -Path $Output
"`nMedians (written $Output):"
$rows | Group-Object Scenario | ForEach-Object {
    $median = { param($values) $s = @($values | Sort-Object); $s[[int](($s.Count - 1) / 2)] }
    [pscustomobject]@{
        Scenario       = $_.Name
        MCyclesPerSec  = & $median $_.Group.MCyclesPerSec
        CycleRange     = "$(($_.Group.MCyclesPerSec | Measure-Object -Minimum).Minimum)-$(($_.Group.MCyclesPerSec | Measure-Object -Maximum).Maximum)"
        SwitchesPerSec = & $median $_.Group.SwitchesPerSec
        SwitchRange    = "$(($_.Group.SwitchesPerSec | Measure-Object -Minimum).Minimum)-$(($_.Group.SwitchesPerSec | Measure-Object -Maximum).Maximum)"
        PaintsPerSec   = & $median $_.Group.PaintsPerSec
    }
} | Format-Table
