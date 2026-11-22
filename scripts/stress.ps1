# Runs the slow stress tests that `cargo test` skips: the fuzzers, the
# engine differential test, and the chaos monkey. Run it before a release.
#
#   .\scripts\stress.ps1                 # about 15 minutes
#   .\scripts\stress.ps1 -Minutes 60     # a longer chaos run
#   .\scripts\stress.ps1 -Cases 20000    # more cases for every fuzzer
#   .\scripts\stress.ps1 -Seed 1234      # replay one chaos seed
#
# A failing fuzzer saves its shrunk input under proptest-regressions/, which
# later runs try first; commit it with the fix. A failing chaos run prints its
# seed, the shortest steps that still fail, and the command that replays it.
param(
    # Minutes for the chaos monkey, split between its two tests.
    [int]$Minutes = 10,
    # Cases for each fuzzer; each one has its own default when this is unset.
    [int]$Cases = 0,
    # Replays one chaos seed instead of running for $Minutes.
    [string]$Seed = '',
    # Steps per chaos run.
    [int]$Steps = 300
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)

if ($Cases -gt 0) { $env:DATADIVER_STRESS_CASES = $Cases } else { Remove-Item Env:DATADIVER_STRESS_CASES -ErrorAction SilentlyContinue }
$env:DATADIVER_CHAOS_STEPS = $Steps
if ($Seed) { $env:DATADIVER_CHAOS_SEED = $Seed } else { Remove-Item Env:DATADIVER_CHAOS_SEED -ErrorAction SilentlyContinue }

$results = [ordered]@{}
function Invoke-Stage([string]$Name, [scriptblock]$Command) {
    Write-Host "`n=== $Name ===" -ForegroundColor Cyan
    $started = Get-Date
    & $Command
    $passed = $LASTEXITCODE -eq 0
    $results[$Name] = '{0} in {1:n0}s' -f ($(if ($passed) { 'passed' } else { 'FAILED' })), ((Get-Date) - $started).TotalSeconds
}

Invoke-Stage 'Build' { cargo test --lib --no-run --quiet }
if (-not $Seed) {
    Invoke-Stage 'Fuzzers and engine differential' { cargo test --lib stress:: -- --ignored }
    Invoke-Stage 'SQL parser properties' { cargo test -p diver_sql --test props --quiet }
}
# The chaos tests replace the panic hook, so they run one at a time.
$chaosSeconds = [math]::Max(1, $Minutes * 60)
$env:DATADIVER_STRESS_SECONDS = [int]($chaosSeconds * 2 / 3)
Invoke-Stage 'Chaos monkey' {
    cargo test --lib -- --ignored --exact ui::dataframe_view::chaos_tests::stress_chaos --nocapture --test-threads=1
}
$env:DATADIVER_STRESS_SECONDS = [int]($chaosSeconds / 3)
Invoke-Stage 'Chaos monkey on damaged profiles' {
    cargo test --lib -- --ignored --exact ui::dataframe_view::chaos_tests::stress_chaos_damaged_profile --nocapture --test-threads=1
}

Write-Host "`n=== Summary ===" -ForegroundColor Cyan
$results.GetEnumerator() | ForEach-Object { '{0,-36} {1}' -f $_.Key, $_.Value }
if ($results.Values -match 'FAILED') { exit 1 }
