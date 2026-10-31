# Slicer discovery

Measured on October 31, 2026, on Windows 11 (build 22631), 24 logical
processors, 64 GB RAM, with release builds.

The Add Slicer picker lists the columns with at most 1,000 distinct values,
with every value. Finding them used to take one HyperLogLog scan over every
column (`approx_count_distinct`), then one exact `SELECT DISTINCT … LIMIT 1001`
query per column estimated at 4,000 values or fewer. The picker showed no
progress while that ran.

## Fixture

100 million rows in a DuckDB file, with 12 columns: `region` (8 values),
`category` (200), `product` (950), `sku` (1,500), `customer_id` (10 million),
`order_id` (unique), `amount` (~1 million), `qty` (50), `day` (730 dates),
`note` (5 million strings), `flag` (2), and `late_rare`, which has 10 values in
its first 99 million rows and 5,000 more in the last million. The same rows
were also read through two views: one over 24 Parquet files, and one joining
the table to a 950-row `dim` table that adds `brand` (40 values). Views rerun
for every query, so they show what each pass costs.

## Strategies

Timed with the DuckDB 1.5.6 CLI, each strategy in one session, median of three
runs. Each strategy found the same eligible columns.

| Strategy | Table | Parquet view | Join view |
|---|---:|---:|---:|
| HLL over all columns, then exact per column (old) | 1.03 s | 1.31 s | 2.68 s |
| Exact per column, no first pass | 5.15 s | 5.24 s | 7.69 s |
| First 100k rows per column, then exact per column | 0.47 s | 0.56 s | 1.73 s |
| First 100k rows, all columns in one query, then exact per column | 0.36 s | 0.41 s | 1.76 s |
| First 1M rows, all columns in one query, then exact per column | 0.47 s | 0.52 s | 1.86 s |
| 1% system sample, all columns, then exact per column | 0.57 s | 6.00 s | 6.88 s |
| First 100k rows, then every remaining column in one `GROUPING SETS` scan | 0.35 s | 0.30 s | 0.67 s |

Notes:

- A column with more than 1,000 distinct values in some rows has more than
  1,000 in all, so checking the first rows rules columns out with certainty.
  `LIMIT` stops the scan early. `late_rare` passes that check and is then ruled
  out by the full read, as it should be.
- Sampling reads every Parquet file and reruns the join, so it is slower than
  reading the first rows.
- Exact per-column queries are cheap on a table, because DuckDB stops the
  hash aggregate under `LIMIT 1001`, but each one reruns a view. One
  `GROUPING SETS` query reads every candidate column in a single pass.
- DuckDB's query progress is in its C API but not in the `duckdb` crate, so
  progress is counted in columns instead. Batching the grouping sets keeps
  progress moving at some cost:

| Columns per scan (join view, 8 candidates) | Time |
|---:|---:|
| 1 | 1.60 s |
| 2 | 0.96 s |
| 4 | 0.67 s |
| 8 | 0.51 s |

## Chosen approach

Discovery checks the first 100,000 rows of every column in one query, then
reads the remaining columns eight at a time with `GROUPING SETS`, keeping at
most 1,001 values per column. The picker shows "N of M checked". Columns
ruled out by the first check count as checked at once.

End to end through `SlicerCatalog::discover` (median of five runs):

| Source | Before | After |
|---|---:|---:|
| Table | 0.69 s | 0.27 s |
| Parquet view | 1.39 s | 0.33 s |
| Join view | 2.41 s | 0.73 s |

Reproduce with a DuckDB file holding the relations:

```powershell
$env:DATADIVER_SLICER_BENCH = "path\to\bench.duckdb"
$env:DATADIVER_SLICER_RELATIONS = "t,pq,joined"
cargo test --release --lib slicer_discovery_benchmark -- --ignored --nocapture
```

The Polars engine holds its frame in memory, so it reads each remaining column
on its own after the same first-rows check.
