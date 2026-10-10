$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class StorageSmokeWindow {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("kernel32.dll")] private static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] private static extern bool AttachThreadInput(uint first, uint second, bool attach);
    [DllImport("user32.dll")] private static extern bool ShowWindow(IntPtr window, int command);
    public static bool Activate(IntPtr window) {
        uint process;
        uint foreground = GetWindowThreadProcessId(GetForegroundWindow(), out process);
        uint current = GetCurrentThreadId();
        bool attached = foreground != current && AttachThreadInput(current, foreground, true);
        try { ShowWindow(window, 9); return SetForegroundWindow(window); }
        finally { if (attached) AttachThreadInput(current, foreground, false); }
    }
    public static uint ProcessForWindow(IntPtr window) { uint process; GetWindowThreadProcessId(window, out process); return process; }
}
'@
$taskPreviousWindow = [StorageSmokeWindow]::GetForegroundWindow()
$taskClipboard = Get-Clipboard -Raw -ErrorAction SilentlyContinue
$taskBinary = (Resolve-Path 'target/release/datadiver.exe').Path
$taskSource = (Resolve-Path 'target/performance/fixtures/10k.csv').Path
$taskProcess = Start-Process -FilePath $taskBinary -ArgumentList @('--file', ('"' + $taskSource + '"'), '--ephemeral', '--no-restore', '--window', '900x600', '--exit-after', '15000') -WindowStyle Normal -PassThru
$taskShell = New-Object -ComObject WScript.Shell
$taskLastCopy = $null
function Send-StorageKey([string]$Keys) {
    $taskProcess.Refresh()
    if ($taskProcess.HasExited) { throw 'The smoke-test window closed unexpectedly' }
    if (-not $taskShell.AppActivate($taskProcess.Id)) { throw 'Cannot focus the smoke-test window' }
    [StorageSmokeWindow]::Activate($taskProcess.MainWindowHandle) | Out-Null
    Start-Sleep -Milliseconds 100
    if ([StorageSmokeWindow]::ProcessForWindow([StorageSmokeWindow]::GetForegroundWindow()) -ne $taskProcess.Id) { throw "Smoke-test window lost focus; no keys sent (target $($taskProcess.Id), foreground $([StorageSmokeWindow]::ProcessForWindow([StorageSmokeWindow]::GetForegroundWindow())), target handle $($taskProcess.MainWindowHandle))" }
    $taskShell.SendKeys($Keys)
    Start-Sleep -Milliseconds 300
}
function Assert-StorageCopy([string]$Expected) {
    Send-StorageKey '^c'
    $taskActual = Get-Clipboard -Raw
    if ($taskActual -ne $Expected) { throw "Clipboard mismatch: expected $Expected, got $taskActual" }
    $script:taskLastCopy = $Expected
}
try {
    $taskProcess.WaitForInputIdle(5000) | Out-Null
    Start-Sleep -Milliseconds 1500
    Assert-StorageCopy '0'
    Send-StorageKey '^r'
    Start-Sleep -Milliseconds 500
    Send-StorageKey '^{END}'
    Assert-StorageCopy '9999'
    Send-StorageKey '^{HOME}'
    Assert-StorageCopy '0'
    Write-Output 'Passed: persistent-source Ctrl+C, Ctrl+R, Ctrl+End, and Ctrl+Home'
} finally {
    if (-not $taskProcess.HasExited) { $taskProcess.CloseMainWindow() | Out-Null; $taskProcess.WaitForExit(10000) | Out-Null }
    if ($null -ne $taskClipboard -and (Get-Clipboard -Raw -ErrorAction SilentlyContinue) -eq $taskLastCopy) { Set-Clipboard -Value $taskClipboard }
    if ($taskPreviousWindow -ne [IntPtr]::Zero) { [StorageSmokeWindow]::SetForegroundWindow($taskPreviousWindow) | Out-Null }
}
