# Cached table widths verification

Verified on Windows on 2026-10-03. The environment reports Intel64 Family 6
Model 183 Stepping 1 and 24 logical processors. No dependencies were added.

## Open and reopen measurements

Debug binary with `snapshots`, 1400×900 window, 100 ms capture delay. Each first
launch used a fresh disposable `--home` profile; the cached launch reused it.
Times cover process launch through capture and shutdown, including source import,
result preparation, painting, and database checkpoint. These are single paired
observations, not isolated inference timings or percentile measurements.

| Fixture | First launch | Cached launch |
|---|---:|---:|
| 10,000 rows × 5 columns | 1,479.58 ms | 1,016.05 ms |
| 100,000 rows × 5 columns | 2,023.17 ms | 1,097.77 ms |
| 1,000,000 rows × 5 columns | 3,817.71 ms | 1,383.42 ms |
| 1,000 rows × 128 columns | 4,263.98 ms | 1,066.18 ms |

The row fixtures use ID, eight regions, amounts, descriptive labels, and a date.
The wide fixture uses 128 string columns and 25 repeating value labels. Fixtures,
profiles, raw timings, and captures are under the ignored `ui/width-*` paths.

To reproduce a pair, launch the snapshot binary twice with the same source and
disposable home, using different capture paths:

```powershell
& .\target\debug\quick_pivot.exe --file ui/width-fixtures/rows-1000000.csv `
    --home ui/width-profiles/benchmark-v2-rows-1000000 `
    --capture ui/reopen.png --window 1400x900 --capture-delay 100 | Out-Null
```

## Bounded work and regressions

- Sample at most 3,000 source positions, projecting only missing-width columns
  in groups of 16, with 250 row IDs per fetch and 201-character SQL previews.
- Retain at most 32 distinct formatted strings per column and measure at most
  four columns per frame, including repeated renders within one frame.
- Snapshot tests assert zero sampling calls when all widths are saved, one
  sampling request when a width is missing, and no additional sampling on search
  or font changes.
- Tests cover real sparse row IDs, Unicode, null/empty samples, manual adjustment
  during pending inference, reset generation, width clamps, text/dialog focus,
  table/view isolation, schema changes, concurrent writer patches, and shutdown
  flushes. Windows destination-lock tests verify failed atomic replacement keeps
  the old file and retries the pending edit.
- Inspected normal, wide, and narrow captures; the narrow grid uses its own
  viewport when a slicer sidebar occupies part of the window.

Shortlisting remains approximate: an unmeasured value can exceed the inferred
width. Cached pixel widths are intentionally retained after data and font changes
until reset.
