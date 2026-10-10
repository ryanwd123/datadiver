# Persistent DuckDB comparison

Measured on 2026-10-03 using five runs per case, matched optimized builds, and generated files. All times below are milliseconds; memory is MiB.

Host: 13th Gen Intel(R) Core(TM) i7-13700K; 16 cores / 24 logical processors; 63.8 GiB RAM; Microsoft Windows 11 Pro 10.0.22631. Profiles and fixtures were on C:. Installed SSDs: TEAM TM8FP4004T; WD_BLACK SN850X 2000GB; Samsung SSD 870 EVO 4TB.

## Loading and memory

Source import/reopen timings end after table access and schema discovery. The cached restart column includes opening the database and startup catalog checks. Same-process reuse is separately recorded in the CSV.

| Fixture | In-memory load | First disk import | Cached restart | Forced reload | Timestamp refresh | Memory after load: memory / import / cached |
|---|---:|---:|---:|---:|---:|---:|
| 10k.csv | 22.5 | 58.3 | 17.4 | 39.9 | 40.9 | 10.3 / 13.1 / 7.4 |
| 100k.csv | 52.2 | 95.2 | 17.3 | 77.2 | 76.5 | 15.3 / 18.1 / 7.5 |
| 1m.csv | 80.8 | 172.5 | 17.1 | 156.2 | 156.9 | 76.5 / 18.2 / 7.4 |
| wide.csv | 382.5 | 621.7 | 18.9 | 591.3 | 600.8 | 130.9 / 33.4 / 8.0 |
| 10k.parquet | 13.8 | 49.6 | 17.0 | 31.8 | 31.2 | 7.6 / 10.2 / 7.4 |
| 100k.parquet | 28.6 | 72.7 | 18.0 | 56.0 | 55.3 | 13.0 / 15.8 / 7.4 |
| 1m.parquet | 28.7 | 111.9 | 17.4 | 92.3 | 94.0 | 76.4 / 16.3 / 7.5 |
| wide.parquet | 168.8 | 388.8 | 18.6 | 364.3 | 367.1 | 125.7 / 24.7 / 8.0 |

## Query timings

Compare the old in-memory source against the persisted source after a cached restart. Each pair is **memory / disk**. Queries retain the current row-map and bounded-page-cache architecture.

| Fixture | Search | Sort | Pivot | Selection statistics |
|---|---:|---:|---:|---:|
| 10k.csv | 4.2 / 3.9 | 2.8 / 3.1 | 14.0 / 4.4 | 6.3 / 6.2 |
| 100k.csv | 25.2 / 20.8 | 19.3 / 19.2 | 15.6 / 4.7 | 15.8 / 15.4 |
| 1m.csv | 43.0 / 36.2 | 102.2 / 107.1 | 16.6 / 5.6 | 59.8 / 62.6 |
| wide.csv | 1031.7 / 1042.3 | 17.7 / 20.9 | 14.9 / 5.4 | 14.4 / 15.6 |
| 10k.parquet | 4.3 / 3.8 | 3.0 / 3.2 | 13.6 / 4.4 | 6.3 / 6.2 |
| 100k.parquet | 25.2 / 20.9 | 18.8 / 19.3 | 15.2 / 4.7 | 15.6 / 15.7 |
| 1m.parquet | 40.9 / 35.7 | 105.5 / 106.6 | 16.8 / 5.7 | 60.2 / 60.4 |
| wide.parquet | 1030.0 / 1036.5 | 17.1 / 20.4 | 14.2 / 5.2 | 13.7 / 15.2 |

## Actual Windows UI

The normal application opened CSV fixtures in a 900×600 window, ran three searches including the existing 50 ms debounce, then scrolled 120 steps. Each cell is **in-memory / first disk import / cached disk restart**. Frame callbacks run after rendering; frame intervals include the benchmark's asynchronous scheduling and are not GPU-only rendering costs. First useful frame starts after window creation, excluding process startup.

| Fixture | First useful frame, median | Search to frame, median | Scroll interval, p95 | Peak private memory | Peak working set |
|---|---:|---:|---:|---:|---:|
| 10k | 96.1 / 131.6 / 96.3 | 81.3 / 83.8 / 88.3 | 10.7 / 10.4 / 10.5 | 95.2 / 129.0 / 92.2 | 75.0 / 88.8 / 74.3 |
| 100k | 149.8 / 181.9 / 111.8 | 103.8 / 106.4 / 99.7 | 10.5 / 10.4 / 10.6 | 150.5 / 153.8 / 94.5 | 85.2 / 95.2 / 78.6 |
| 1m | 236.0 / 323.9 / 185.7 | 142.4 / 133.2 / 131.6 | 10.5 / 10.4 / 10.7 | 210.0 / 224.1 / 140.9 | 198.7 / 171.3 / 142.0 |
| wide | 487.2 / 705.6 / 127.4 | 1053.8 / 1037.7 / 1042.5 | 10.5 / 10.7 / 10.8 | 488.5 / 491.7 / 128.3 | 369.8 / 370.4 / 98.3 |

## Disk footprint and interpretation

| Fixture | Source file, MiB | Database after refresh/cleanup, MiB |
|---|---:|---:|
| 10k.csv | 0.31 | 3.01 |
| 100k.csv | 3.33 | 3.51 |
| 1m.csv | 35.03 | 13.01 |
| wide.csv | 45.99 | 22.26 |
| 10k.parquet | 0.11 | 3.01 |
| 100k.parquet | 0.94 | 3.51 |
| 1m.parquet | 9.32 | 12.51 |
| wide.parquet | 1.38 | 22.26 |

- Cached access avoids parsing the source. Import/refresh still reads the complete file and writes durable data.
- DuckDB still uses memory for buffered database pages, temporary pivots, row maps, row-ID vectors, and display caches. On-disk storage does not guarantee lower memory use for every operation.
- Pivot timings include removing the old Parquet transfer into a separate in-memory database; they reflect that implementation change as well as the source storage change.
- Files have five mixed-type columns at 10K, 100K, and 1M rows. The wide fixture has 100 columns and 100K rows. Timestamp refresh changes the source timestamp by ten seconds; values are unchanged, so it measures the full-refresh path without altering the workload.
- Operating-system file caches were not flushed. First database import is not a claim of cold physical disk I/O. Five timing samples give a useful median; query p95 in the CSV is the maximum of those five samples. Each UI scroll p95 uses 600 intervals.
- Old table revisions can occupy additional disk space while windows/jobs retain them. Retired tables are dropped after their snapshots release; physical database allocation can remain reusable space.

## Verification and reproduction

Passed: 99 normal tests, 125 snapshot/UI tests, Clippy with warnings denied in both configurations, formatting, and the normal release build. Windows OS clipboard/reload/navigation checks passed; inspected CSV, TSV, XLSX, information, and pivot captures showed the expected data. Regression coverage includes refresh/reuse, modification time and size, duplicates/nulls, schema changes, source `ROWID`, failed imports, file changes during import, catalog timestamps, missing-source cleanup, and independent windows.

Database location: `<settings folder>/quick_pivot.duckdb`. Current source metadata is in `main.__qp_file_catalog`; source data tables are `files.file_<id>_v<revision>`. Pivot results and display row maps remain temporary. Normal access compares source modification time (nanoseconds) and file size with the recorded import stamp. Changes that preserve both require Ctrl+R or Ctrl+Shift+R, which force a fresh import. Failed imports preserve the previous snapshot; missing sources remove their catalog entry and retire cached tables after active snapshots release them.

The generated-data benchmark is `examples/storage_benchmark.rs`. Run `cargo build --release --example storage_benchmark --bin quick_pivot`, generate fixtures with `target/release/examples/storage_benchmark.exe generate target/performance/fixtures`, then run `scripts/measure-storage.ps1` and `scripts/measure-storage-ui.ps1 -Binary target/release/quick_pivot.exe -Backend persistent`. UI measurement windows must be visible for Windows to deliver drawing frames. Restart comparisons use disposable `--home` profiles; ordinary screenshot runs use `--ephemeral`.

The preserved pre-change executables are `target/performance/baseline-storage.exe` and `baseline-ui.exe`. The baseline used the same generated data and measurement workload; its source queries always loaded into a connection-local temporary table. Re-running the baseline requires those preserved executables or the pre-change code.

Aggregated measurements: [storage-results.csv](storage-results.csv). Raw measurements and hardware metadata: `target/performance/*.json` and `*.jsonl`; archived in `target/performance/raw-results.zip`. Summarize again with `python scripts/summarize-storage.py`.
