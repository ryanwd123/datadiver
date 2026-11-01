# Performance Review

> **For the agent:** When asked to "work on performance.md", follow the instructions below. Each session covers **one** focused performance investigation. Record the findings in **Performance Notes** and add any recommended improvements to **To Do**.

## Instructions

### 1. Orient
- Read this entire file first, especially the **Coverage** table and **Performance Notes**, so you know what has already been evaluated.
- Skim the codebase to see what has changed since the most recent entry. Run `git log` since the commit recorded in that entry.

### 2. Pick a target
- Choose **one** area that has not been evaluated yet. You may revisit an evaluated area only if the code has changed significantly since its entry, or if its entry says to re-check under certain conditions.
- Prefer areas a user would notice: idle CPU, UI responsiveness, startup, and large-data handling.
- State which area you picked and why before you start measuring.

Candidate areas (extend this list as the app grows):
- Idle CPU and wakeups on Windows (the goal is near-zero idle CPU)
- Frame/render time during scrolling, resizing, and large table views
- Startup time, from cold start to first interactive frame
- Data loading and parsing (file open, schema inference, type conversion)
- Query and filter execution on large datasets
- Memory: peak usage, steady-state usage, and leaks over long sessions
- Allocations and clones in hot paths
- Background work, threading, and contention
- Text layout and shaping costs
- Binary size and dependency weight

### 3. Measure a baseline
- Always use a **release** build. Record the commit hash, OS, CPU, and dataset size.
- Pick suitable tools. Some options:
  - `criterion` benchmarks in `benches/` for isolated functions
  - `hyperfine` for startup and CLI-style timings
  - Windows Performance Recorder/Analyzer (ETW) or `superluminal`/`samply` for CPU profiles and wakeups
  - Process Explorer for idle CPU and context switches
  - `dhat` or a counting allocator for allocations
- Run each measurement enough times to get stable numbers. Report the median and the spread.

### 4. Explore approaches
- Try at least 2–3 alternative approaches where that makes sense. Benchmark each one under the same conditions as the baseline.
- Do the experiments on a scratch branch or in benchmark code only. **Do not merge changes to app code.** Fixes go into **To Do** for me to approve.
- Commit reusable benchmark harnesses in `benches/` so future sessions can re-run them.

### 5. Record
- Add a new entry at the **top** of **Performance Notes** using the template below.
- Update the **Coverage** table.
- Add worthwhile improvements to **To Do**, with the expected impact and a link back to the note.
- If nothing worth improving turned up, record that too. A "this is already fast" result keeps future sessions from repeating the investigation.

### Housekeeping
- Keep entries concise: numbers over prose.
- When this file gets long, condense entries older than a few months into one-line summaries. Keep the Coverage table complete.

---

## Coverage

| Area | Last evaluated | Commit | Result | Re-check when |
|------|----------------|--------|--------|---------------|
| Idle CPU and wakeups (Windows) | 2026-10-31 | `c00a9fd` | Near zero except the SQL caret blink (full-window repaint, ~20 Mcycles/s) | The blink, source monitor, timers or render target change |
| Storage: in-memory vs persisted DuckDB | 2026-10-03 | `8df5ccb` | Persisted; see `performance/STORAGE.md` | The source database or row-map design changes |
| Query modes: staged vs direct pivot | 2026-10-03 | `8df5ccb` | Direct; see `performance/QUERY_MODES.md` | The pivot pipeline changes |
| Cached table widths | 2026-10-03 | `8df5ccb` | Cached; see `performance/TABLE_WIDTHS.md` | Width sampling changes |
| Slicer discovery | 2026-10-31 | `43ee4c3` | GROUPING SETS scan; see `performance/SLICER_DISCOVERY.md` | The slicer picker's queries change |

---

## Performance Notes

<!--
Entry template:

### YYYY-MM-DD — <Area>
**Commit:** `abc1234` · **Build:** release · **Machine:** <CPU / RAM / OS>
**Question:** What was being investigated and why.
**Method:** Tools, dataset, how it was measured.

| Approach | Metric | Result | vs. baseline |
|----------|--------|--------|--------------|
| Baseline | | | — |
| A: ... | | | |
| B: ... | | | |

**Findings:** Key takeaways.
**Recommendation:** What to do (or "no action needed").
**Re-check when:** Conditions that would make this worth revisiting.
-->

### 2026-10-31 — Idle CPU and wakeups (Windows)
**Commit:** `c00a9fd` · **Build:** release · **Machine:** i7-13700K (24 logical) / 64 GB / Windows 11 Pro 22631 / RTX 3090, driver 32.0.16.1088
**Question:** Does the app stay near zero CPU while idle? Recent commits added an animated busy indicator and timed toasts, and a 1 s source monitor always runs.
**Method:** `benches/idle_cpu.ps1`: start the app with `--ephemeral --no-restore --window 1200x800`, settle 8 s, then measure 20 s untouched. CPU from `QueryProcessCycleTime`, since `TotalProcessorTime` is tick-sampled and read 0 in every run. Context switches come from the per-thread perf counters, grouped by the module each thread started in. Paints come from a stderr trace. Data: `--rows 1000000`, or a 100k-row CSV (`--file`). 3 runs per row; median (range). Experiments are env switches on the local branch `perf/idle-experiments` (`38a43c9`). For active-use cost, `--benchmark-ui` was run 3× per case, comparing total process cycles.

| Scenario / approach | Mcycles/s | Switches/s (driver + app) | Paints/s | vs. baseline |
|---|---:|---:|---:|---|
| Table, synthetic rows | 2.9 (2.8–2.9) | 65 (64 + 1) | 0 | — |
| Table, CSV file | 4.5 (4.4–4.7) | 69 (64 + 5) | 0 | — |
| Database tree open | 4.6 (4.1–5.3) | 69 (64 + 5) | 0 | — |
| **SQL editor focused** | **20.6 (19.9–21.8)** | **90 (64 + 26)** | **1.87** | — |
| SQL editor focused, minimized | 3.6 (2.5–3.8) | 68 (63 + 5) | 0 | — |
| A: software Direct2D, table | 0.29 (0.19–0.37) | 1.3 (0 + 1.3) | 0 | −90% idle |
| A: software Direct2D, SQL editor | 33.1 (31.2–34.1) | 53 | 1.87 | +61% |
| B: release the render target while minimized | 2.9 | 65 (63 + 1) | 0 | no change |
| C: no source monitor, CSV file | 2.9 (2.6–3.0) | 64 (64 + 0) | 0 | −1.6 Mcycles/s, −5 switches/s |
| C: no source monitor, database tree | 2.6 (2.6–2.8) | 64 (64 + 0) | 0 | −2.0 Mcycles/s, −5 switches/s |
| D: no caret blink, SQL editor | 3.2 (3.2–3.4) | 65 (64 + 1.3) | 0 | −84% |
| D: no caret blink, SQL editor minimized | 3.1 (3.1–3.1) | 64 (63 + 1.4) | 0 | −3.5 switches/s |
| E: clip the paint to the update rect, SQL editor | 19.8 (17.3–20.3) | 91 | 1.88 | no real change |

Active use, whole `--benchmark-ui` run, 243–252 paints, total Gcycles: hardware 2.79 (1200×800) and 2.66 (3000×1500); software 3.90 (+40%) and 4.87 (+83%).

**Findings:**
- **No idle repaints in the table:** the busy indicator and toast timers stop once work finishes. The event loop sleeps in `MsgWaitForMultipleObjectsEx` until the next deadline.
- **64 wakeups/s come from one NVIDIA driver thread** (`nvwgf2umx.dll`, 16 threads in all). It runs as long as a hardware D2D target exists, minimized or not, and costs ~2.6 Mcycles/s. A target dropped while minimized doesn't stop it. Software rendering removes it, but costs about 4.5–9 Mcycles more per paint, so it loses as soon as the app is used. That's driver behavior, not something to fix.
- **The SQL caret blink is the one real idle cost.** The hosted editor invalidates only its own rectangle, but `render()` builds and presents the whole window: about 9 Mcycles and 13 main-thread switches per blink, every 530 ms. That is ~5× everything else combined (≈0.5% of one core). Clipping the D2D draw (E) saves nothing, so the cost is in building the frame on the CPU, not in rasterizing it.
- **The blink timer keeps firing while the window is minimized** (WM_TIMER without WM_PAINT): +3.5 switches/s.
- **The 1 s source monitor** costs 1.6–2.0 Mcycles/s and ~5 switches/s with a file or the database tree: the UI timer, a pool-thread probe, and the wake message.
- **Hardware `EndDraw` waits on present.** Under continuous painting it takes ~10.7 ms per paint (p50) at both window sizes, against 2.1 ms for an isolated blink paint and 2–3 ms in software. With `D2D1_PRESENT_OPTIONS_NONE`, the UI thread blocks in present for most of each scroll frame. This is outside this session's area: see To Do.

**Recommendation:** Stop the blink from repainting the whole window while nothing is happening (To Do 1–2). Pause the source monitor while the window is inactive (To Do 3). Leave the GPU path as it is.
**Re-check when:** the caret, any repeating timer, the render target or present options change, or another animation is added. Re-run on a machine with an Intel or AMD GPU, since the driver thread is NVIDIA-specific.


---

## To Do

<!--
Format: - [ ] <Improvement> — expected impact: <e.g. ~40% faster file open> — see note YYYY-MM-DD <Area>
When done: - [x] ... (done YYYY-MM-DD, commit abc1234)
Keep completed items as history; don't delete them.
-->

- [ ] Stop the SQL caret blinking after ~10 s with no input in the editor (caret stays solid; GTK's default is 10 s) and restart it on the next key, click or focus. Expected impact: SQL-editor idle 20.6 → ~3.2 Mcycles/s and 90 → 65 switches/s after the timeout, with blinking unchanged while typing. — see note 2026-10-31 Idle CPU
- [ ] Kill the editor's blink timer while the host window is minimized (`IsIconic`) and restart it on restore. Expected impact: −3.5 switches/s minimized with the SQL editor focused. — see note 2026-10-31 Idle CPU
- [ ] Optional, bigger: make a blink repaint only the editor, e.g. keep the rest of the frame in a cached bitmap, or skip widgets outside the update rect. Expected impact: ~9 → ≤1 Mcycles per blink, and cheaper paints for anything else that invalidates a small area. Clipping alone does not help (experiment E). — see note 2026-10-31 Idle CPU
- [ ] Pause the 1 s source monitor while the window is inactive and run one check on activation, or watch the file with `FindFirstChangeNotificationW`. Expected impact: −1.6 to −2.0 Mcycles/s and −5 switches/s for a background window with a file or the database tree open. — see note 2026-10-31 Idle CPU
- [ ] Lead for a "Frame/render time" session: hardware `EndDraw` takes ~10.7 ms per paint when painting continuously (vs 2–3 ms of drawing). Measure input-to-frame latency while scrolling and typing with `D2D1_PRESENT_OPTIONS_IMMEDIATELY`, or with a flip-model swap chain and a frame-latency waitable object. — see note 2026-10-31 Idle CPU