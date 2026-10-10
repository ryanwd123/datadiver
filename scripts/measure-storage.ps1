param([int]$Repeats = 5)
$ErrorActionPreference = 'Stop'
$taskRoot = (Resolve-Path 'target/performance').Path
$taskBinary = (Resolve-Path 'target/release/examples/storage_benchmark.exe').Path
foreach ($fixture in @('10k.csv','100k.csv','1m.csv','wide.csv','10k.parquet','100k.parquet','1m.parquet','wide.parquet')) {
    foreach ($repeat in 1..$Repeats) {
        $taskProfile = Join-Path $taskRoot ('storage-profiles/' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $taskProfile | Out-Null
        $taskSource = Join-Path $taskProfile $fixture
        Copy-Item -LiteralPath (Join-Path $taskRoot "fixtures/$fixture") -Destination $taskSource
        foreach ($phase in @('import','reopen','force','refresh')) {
            if ($phase -eq 'refresh') {
                $taskModified = [System.IO.File]::GetLastWriteTimeUtc($taskSource).AddSeconds(10)
                [System.IO.File]::SetLastWriteTimeUtc($taskSource, $taskModified)
            }
            $taskMode = if ($phase -eq 'force') { 'force' } else { 'persistent' }
            & $taskBinary $taskMode $taskSource (Join-Path $taskProfile 'config') | Set-Content -LiteralPath (Join-Path $taskRoot "persistent-$phase-$fixture-$repeat.jsonl")
            if ($LASTEXITCODE -ne 0) { throw "Storage benchmark failed: $fixture $phase" }
        }
        Get-Item -LiteralPath (Join-Path $taskProfile 'config/quick_pivot.duckdb') | Select-Object Length | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $taskRoot "database-size-$fixture-$repeat.json")
    }
}
