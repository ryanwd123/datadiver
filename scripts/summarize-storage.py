"""Summarize the five-run storage and real-window comparison without dependencies."""
import csv
import json
import math
import statistics
from pathlib import Path

ROOT = Path("target/performance")
OUT = Path("performance")
OUT.mkdir(exist_ok=True)


def read(path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def p95(values):
    return sorted(values)[math.ceil(len(values) * .95) - 1]


def storage(backend, fixture):
    prefix = "baseline" if backend == "baseline" else f"persistent-{backend}"
    return [[json.loads(line) for line in (ROOT / f"{prefix}-{fixture}-{repeat}.jsonl").read_text(encoding="utf-8-sig").splitlines()] for repeat in range(1, 6)]


def median_metric(runs, name):
    return statistics.median(event["ms"] for run in runs for event in run if event["metric"] == name)


def memory_metric(runs, name, field):
    return statistics.median(event["memory"][field] / 1024 ** 2 for run in runs for event in run if event["metric"] == name)


rows = []
for fixture in (f"{size}.{extension}" for extension in ["csv", "parquet"] for size in ["10k", "100k", "1m", "wide"]):
    for phase in ["baseline", "import", "reopen", "force", "refresh"]:
        runs = storage(phase, fixture)
        for metric in ["load", "all_rows", "search", "sort", "page", "pivot", "statistics"] + ([] if phase == "baseline" else ["cached_same_process"]):
            values = [event["ms"] for run in runs for event in run if event["metric"] == metric]
            rows.append({"kind": "storage", "fixture": fixture, "phase": phase, "metric": metric, "n": len(values), "median_ms": statistics.median(values), "p95_ms": p95(values), "private_mib": memory_metric(runs, metric, "private_bytes"), "working_set_mib": memory_metric(runs, metric, "working_set"), "peak_working_set_mib": memory_metric(runs, metric, "peak_working_set")})

ui_rows = []
for fixture in ["10k", "100k", "1m", "wide"]:
    for backend, phase in [("baseline", "import"), ("persistent", "import"), ("persistent", "reopen")]:
        runs = [read(ROOT / f"{backend}-ui-{fixture}-{repeat}-{phase}.json") for repeat in range(1, 6)]
        memory = [read(ROOT / f"{backend}-ui-{fixture}-{repeat}-{phase}.json.memory.json") for repeat in range(1, 6)]
        for metric in ["first_useful_frame", "search_to_frame", "scroll_frame_interval"]:
            values = [event["ms"] for run in runs for event in run if event["metric"] == metric]
            ui_rows.append({"kind": "ui", "fixture": fixture, "phase": f"{backend}-{phase}", "metric": metric, "n": len(values), "median_ms": statistics.median(values), "p95_ms": p95(values), "private_mib": statistics.median(max(sample["private_bytes"] for sample in run) / 1024 ** 2 for run in memory), "working_set_mib": statistics.median(max(sample["working_set"] for sample in run) / 1024 ** 2 for run in memory), "peak_working_set_mib": statistics.median(max(sample["peak_working_set"] for sample in run) / 1024 ** 2 for run in memory)})

with (OUT / "storage-results.csv").open("w", newline="", encoding="utf-8") as stream:
    writer = csv.DictWriter(stream, fieldnames=list(rows[0]))
    writer.writeheader()
    writer.writerows(rows + ui_rows)

cpu, ram, os = (read(ROOT / name) for name in ["cpu.json", "ram.json", "os.json"])
report = [
    "# Persistent DuckDB comparison", "",
    "Measured on 2026-10-03 using five runs per case, matched optimized builds, and generated files. All times below are milliseconds; memory is MiB.", "",
    f"Host: {cpu['Name']}; {cpu['NumberOfCores']} cores / {cpu['NumberOfLogicalProcessors']} logical processors; {ram['TotalPhysicalMemory'] / 1024 ** 3:.1f} GiB RAM; {os['Caption']} {os['Version']}. Profiles and fixtures were on C:. Installed SSDs: " + "; ".join(disk["FriendlyName"] for disk in read(ROOT / "disks.json")) + ".", "",
    "## Loading and memory", "",
    "Source import/reopen timings end after table access and schema discovery. The cached restart column includes opening the database and startup catalog checks. Same-process reuse is separately recorded in the CSV.", "",
    "| Fixture | In-memory load | First disk import | Cached restart | Forced reload | Timestamp refresh | Memory after load: memory / import / cached |",
    "|---|---:|---:|---:|---:|---:|---:|",
]
for fixture in (f"{size}.{extension}" for extension in ["csv", "parquet"] for size in ["10k", "100k", "1m", "wide"]):
    runs = {phase: storage(phase, fixture) for phase in ["baseline", "import", "reopen", "force", "refresh"]}
    times = " | ".join(f"{median_metric(runs[phase], 'load'):.1f}" for phase in runs)
    memory = " / ".join(f"{memory_metric(runs[phase], 'load', 'private_bytes'):.1f}" for phase in ["baseline", "import", "reopen"])
    report.append(f"| {fixture} | {times} | {memory} |")

report += ["", "## Query timings", "", "Compare the old in-memory source against the persisted source after a cached restart. Each pair is **memory / disk**. Queries retain the current row-map and bounded-page-cache architecture.", "", "| Fixture | Search | Sort | Pivot | Selection statistics |", "|---|---:|---:|---:|---:|"]
for fixture in (f"{size}.{extension}" for extension in ["csv", "parquet"] for size in ["10k", "100k", "1m", "wide"]):
    before, after = storage("baseline", fixture), storage("reopen", fixture)
    values = " | ".join(f"{median_metric(before, metric):.1f} / {median_metric(after, metric):.1f}" for metric in ["search", "sort", "pivot", "statistics"])
    report.append(f"| {fixture} | {values} |")

report += ["", "## Actual Windows UI", "", "The normal application opened CSV fixtures in a 900×600 window, ran three searches including the existing 50 ms debounce, then scrolled 120 steps. Each cell is **in-memory / first disk import / cached disk restart**. Frame callbacks run after rendering; frame intervals include the benchmark's asynchronous scheduling and are not GPU-only rendering costs. First useful frame starts after window creation, excluding process startup.", "", "| Fixture | First useful frame, median | Search to frame, median | Scroll interval, p95 | Peak private memory | Peak working set |", "|---|---:|---:|---:|---:|---:|"]
for fixture in ["10k", "100k", "1m", "wide"]:
    phases = ["baseline-import", "persistent-import", "persistent-reopen"]
    def ui(metric, column):
        return " / ".join(f"{next(row[column] for row in ui_rows if row['fixture'] == fixture and row['phase'] == phase and row['metric'] == metric):.1f}" for phase in phases)
    report.append(f"| {fixture} | {ui('first_useful_frame', 'median_ms')} | {ui('search_to_frame', 'median_ms')} | {ui('scroll_frame_interval', 'p95_ms')} | {ui('first_useful_frame', 'private_mib')} | {ui('first_useful_frame', 'peak_working_set_mib')} |")

report += ["", "## Disk footprint and interpretation", "", "| Fixture | Source file, MiB | Database after refresh/cleanup, MiB |", "|---|---:|---:|"]
for fixture in (f"{size}.{extension}" for extension in ["csv", "parquet"] for size in ["10k", "100k", "1m", "wide"]):
    sizes = [read(ROOT / f"database-size-{fixture}-{repeat}.json")["Length"] for repeat in range(1, 6)]
    report.append(f"| {fixture} | {(ROOT / 'fixtures' / fixture).stat().st_size / 1024 ** 2:.2f} | {statistics.median(sizes) / 1024 ** 2:.2f} |")
report += ["", "- Cached access avoids parsing the source. Import/refresh still reads the complete file and writes durable data.", "- DuckDB still uses memory for buffered database pages, temporary pivots, row maps, row-ID vectors, and display caches. On-disk storage does not guarantee lower memory use for every operation.", "- Pivot timings include removing the old Parquet transfer into a separate in-memory database; they reflect that implementation change as well as the source storage change.", "- Files have five mixed-type columns at 10K, 100K, and 1M rows. The wide fixture has 100 columns and 100K rows. Timestamp refresh changes the source timestamp by ten seconds; values are unchanged, so it measures the full-refresh path without altering the workload.", "- Operating-system file caches were not flushed. First database import is not a claim of cold physical disk I/O. Five timing samples give a useful median; query p95 in the CSV is the maximum of those five samples. Each UI scroll p95 uses 600 intervals.", "- Old table revisions can occupy additional disk space while windows/jobs retain them. Retired tables are dropped after their snapshots release; physical database allocation can remain reusable space.", "", "## Verification and reproduction", "", "Passed: 99 normal tests, 125 snapshot/UI tests, Clippy with warnings denied in both configurations, formatting, and the normal release build. Windows OS clipboard/reload/navigation checks passed; inspected CSV, TSV, XLSX, information, and pivot captures showed the expected data. Regression coverage includes refresh/reuse, modification time and size, duplicates/nulls, schema changes, source `ROWID`, failed imports, file changes during import, catalog timestamps, missing-source cleanup, and independent windows.", "", "Database location: `<settings folder>/quick_pivot.duckdb`. Current source metadata is in `main.__qp_file_catalog`; source data tables are `files.file_<id>_v<revision>`. Pivot results and display row maps remain temporary. Normal access compares source modification time (nanoseconds) and file size with the recorded import stamp. Changes that preserve both require Ctrl+R or Ctrl+Shift+R, which force a fresh import. Failed imports preserve the previous snapshot; missing sources remove their catalog entry and retire cached tables after active snapshots release them.", "", "The generated-data benchmark is `examples/storage_benchmark.rs`. Run `cargo build --release --example storage_benchmark --bin datadiver`, generate fixtures with `target/release/examples/storage_benchmark.exe generate target/performance/fixtures`, then run `scripts/measure-storage.ps1` and `scripts/measure-storage-ui.ps1 -Binary target/release/datadiver.exe -Backend persistent`. UI measurement windows must be visible for Windows to deliver drawing frames. Restart comparisons use disposable `--home` profiles; ordinary screenshot runs use `--ephemeral`.", "", "The preserved pre-change executables are `target/performance/baseline-storage.exe` and `baseline-ui.exe`. The baseline used the same generated data and measurement workload; its source queries always loaded into a connection-local temporary table. Re-running the baseline requires those preserved executables or the pre-change code.", "", "Aggregated measurements: [storage-results.csv](storage-results.csv). Raw measurements and hardware metadata: `target/performance/*.json` and `*.jsonl`; archived in `target/performance/raw-results.zip`. Summarize again with `python scripts/summarize-storage.py`.", ""]
(OUT / "STORAGE.md").write_text("\n".join(report), encoding="utf-8")
print("Wrote performance/STORAGE.md and performance/storage-results.csv")
