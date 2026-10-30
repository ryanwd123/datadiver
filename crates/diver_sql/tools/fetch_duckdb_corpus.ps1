# Sparse-checks out test/sql of DuckDB v1.5.6 into crates/diver_sql/corpus/duckdb
# (gitignored), for the corpus test:
#
#     cargo test -p diver_sql --test duckdb_corpus -- --ignored --nocapture

$ErrorActionPreference = 'Stop'
$tag = 'v1.5.6'
$dest = Join-Path $PSScriptRoot '..\corpus\duckdb'

if (Test-Path (Join-Path $dest 'test\sql')) {
    Write-Host "already there: $dest"
    exit 0
}
New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
git clone --depth 1 --branch $tag --filter=blob:none --sparse https://github.com/duckdb/duckdb.git $dest
git -C $dest sparse-checkout set test/sql
Write-Host "fetched $tag test/sql into $dest"
