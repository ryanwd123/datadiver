# Query mode verification

Measured on October 3, 2026, on Windows 11 (build 22631), Intel Core
i7-13700K, 24 logical processors. RAM capacity was unavailable through the
sandbox's hardware inventory interface.

Run the reproducible comparison with:

```powershell
cargo test --lib filtered_pivot_performance_comparison -- --ignored --nocapture
```

The fixture has 24 columns: Year, Sex, Name, Season, and 20 repeated payload
strings. One third of rows match Winter. Both paths group by Year, pivot Sex,
count Name, order by Year, and cap output at 100,000 rows. The old path copies
the filtered source and two helper columns into a temporary input table; the
new path discovers dimension labels and pivots the source directly. The test
asserts identical schemas and all output values.

These are local debug-build medians of three iterations following one warm-up.
Both paths include result materialization and the existing in-memory result
transport, and the staged path includes its input-table cleanup.

| Source rows | Direct query | Staged input |
|---:|---:|---:|
| 10,000 | 42.995 ms | 85.178 ms |
| 100,000 | 71.303 ms | 467.908 ms |
| 1,000,000 | 84.431 ms | 1,750.357 ms |

The benchmark uses deterministic in-memory source tables and a deliberately
wide payload. These measurements demonstrate removal of staging cost on this
fixture; they are not release-build or end-to-end file-loading timings. LIMIT
bounds the result, not the number of source rows scanned by aggregation or
dimension discovery.

## Acceptance checks

- Full suite: 133 passing tests without snapshots; 178 with snapshots. The
  performance comparison is ignored in routine runs and was run separately.
- Clippy passed for all targets in both configurations with warnings denied.
- Inspected captures for normal and 640-pixel query headers, pivot setup
  guidance, pivots without measures, and SELECT/PIVOT Information SQL. Narrow
  headers hide the modified timestamp and truncate the document title.
- The optimized binary compiled and passed an ephemeral query-view startup
  smoke test without snapshots. The existing `target/release/quick_pivot.exe`
  was running and could not be replaced; the new executable is available as
  `target/release/quick_pivot-query-modes.exe`. The release library build passed.
