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
| Frame time and input-to-screen latency (scrolling) | 2026-11-21 | `bd31a2a` | A frame costs ~0.5 ms to build; the latency is present queueing. Wheel input takes 29 ms to reach the screen, 6.5 ms with `PRESENT_OPTIONS_IMMEDIATELY` | The present path, render target or message loop changes, or a view gets much heavier to paint |

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

### 2026-11-21 — Frame time and input-to-screen latency while scrolling
**Commit:** `bd31a2a` · **Build:** release (`perf` profile = release, built into `target/perf`) · **Machine:** i7-13700K / 64 GB / Windows 11 Pro 22631 / RTX 3090 (driver 32.0.16.1088), one 3840×1600 display at **84 Hz** (11.9 ms per refresh)
**Question:** The idle-CPU note found hardware `EndDraw` taking ~10.7 ms per paint under continuous painting. Is the UI thread blocked on present, how long does input take to reach the screen, and do other present paths do better?
**Method:** `benches/frame_latency.ps1` drives `benches/frame_probe`. It starts the app (`--rows 1000000 --ephemeral --no-restore`) and settles 6 s. The probe then posts input to the window at a fixed rate for 5 s and records each post's QPC time. With `DD_FRAME_TRACE` the app draws its frame number as an 18-cell marker in the client corner and logs QPC times for each frame and each dispatched input. The probe reads the marker off the screen with Desktop Duplication, whose `LastPresentTime` is when DWM put the frame on the desktop, not the moment light leaves the panel. Every input is matched 1:1 to the first frame that drew it (600/600 or 150/150 in every run, 0 bad markers). Scenarios: wheel at 120 Hz (a fast spin or a touchpad) in a 1200×800 window and in a 3600×1400 window, and Down arrow at 30 Hz (a held key). All variants are env switches in `performance/frame-experiments.patch`. Values are the median of 3 runs (range of the run medians). Mcycles/s is the whole process during the 5 s of input.

| Variant | Wheel 120 Hz: to screen p50 / p95 (ms) | fps drawn / shown | Mcycles/s | Key 30 Hz: to screen p50 / p95 (ms) | vs. baseline |
|---|---:|---:|---:|---:|---|
| Baseline: HWND target, `PRESENT_OPTIONS_NONE` | **29.1** (29.0–29.1) / 34.4 | 85 / 85 | 152–191 | 3.8–5.0 / 4.4–12.6 | — |
| A: `PRESENT_OPTIONS_IMMEDIATELY` | **6.5** (6.4–6.6) / 11.8 | 121 / 85 | 211–227 | 5.2 (4.2–5.9) / 12.2 | wheel −78%; key same |
| B: flip swap chain, `Present(1)` | 53.5 (52.7–56.8) / 59.2 | 85 / 84 | 344–398 | 1.8 (1.7–8.2) / 13.3 | wheel +84% |
| C: flip, `Present(0)` | 52.9 (52.8–53.0) / 58.4 | 85 / 84 | 347–400 | 2.3 / 4.9 | wheel +82% |
| D: flip, waitable (latency 1) waited at paint start | 29.3 (29.3–29.3) / 34.7 | 85 / 85 | 208–212 | 1.7 (1.6–3.4) / 2.2 | wheel same; key −60% |
| **E: flip, waitable waited in the message loop** | **18.1** (18.1–18.1) / 23.5 | 85 / 84 | 250–254 | **1.8** (1.8–1.8) / **2.3** | wheel −38%; key −60% p50, −80% p95 |
| E0: as E, `Present(0)` | 17.9 (17.9–18.0) / 23.4 | 85 / 85 | 218–224 | 1.9 (1.9–4.2) / 5.0 | as E |

Large window (3600×1400), wheel 120 Hz, to screen p50: baseline 29.1, A 6.6 (6.4–6.9), B 52.9, C 53.1, D 29.4, E 18.2, E0 18.1. Per-frame CPU at either size: about 0.2 ms to record the paint plus 0.15–0.3 ms of `EndDraw` when it doesn't block. Idle (`benches/idle_cpu.ps1 -Only table`, same exe): baseline 2.9, E 2.6 and A 3.3 Mcycles/s, all within run-to-run spread, with the same 64/s NVIDIA thread and no paints.

**Findings:**
- **Building a frame is cheap:** ~0.5 ms of CPU even at 3600×1400. The latency comes from queueing, not drawing.
- **Baseline: under continuous input the UI thread spends ~97% of each frame blocked in `EndDraw`** (11.5 ms of the 11.9 ms refresh), because present waits for vblank. Input posted during that wait isn't dispatched until the wait ends: 17.5 ms from post to submitted frame, and 29 ms (≈2.4 refreshes) to the screen. Isolated key presses don't hit this: 4–5 ms p50, though p95 reaches 12.6 ms when a press just misses a composition.
- **A (`IMMEDIATELY`) is a one-line change that removes the wait.** Continuous wheel input reaches the screen in 6.5 ms, about half a refresh, which is the floor for DWM-composed output. Because nothing paces it, it draws a frame for every input batch: 121 fps drawn for 85 shown, so ~30% of frames are wasted, at +15–40% CPU while scrolling. Its frame rate follows the input rate, so a 500–1000 Hz input source (touchpad, drag) needs a cap. No tearing appeared (0 bad markers), but the 6 px marker can only catch tearing along its own strip.
- **Flip model without a correct wait is worse:** B and C queue three frames (53 ms). D only moves the block from `EndDraw` to the start of `render()`, which runs after input is dispatched, so it matches the baseline.
- **E (wait in `MsgWaitForMultipleObjectsEx`, paint only when signaled) gives the lowest latency for isolated input:** 1.8 ms p50 and 2.3 ms p95, with no wasted frames. Under continuous input it is still one refresh behind (18 ms), since the frame starts at the vblank that frees the buffer and shows at the next. It also costs the most to build: a D3D11 device and DXGI swap chain, resize and device-lost handling, and a message-loop change. Process CPU while scrolling is ~+40% over the baseline at the same 85 fps.
- **The `--benchmark-ui` metric `scroll_frame_interval` doesn't measure present:** its `Step::Frame` polls with 5 ms timers. Use `benches/frame_latency.ps1` for this question.

**Recommendation:** Switch the HWND target to `D2D1_PRESENT_OPTIONS_IMMEDIATELY` and cap painting at about one frame per 4 ms (To Do 6). This is the biggest win for scrolling (−78%) for one line plus a small guard, and it changes nothing else measured. A flip-model swap chain with the waitable in the message loop is the better design for clicks and keys, but it's a much bigger change and slower than A for continuous scrolling. Keep it as an option (To Do 7).
**Re-check when:** the present path, render target or message loop changes, or a view starts taking >2 ms to paint. Re-run at 60 Hz and on an Intel or AMD GPU, since the numbers depend on the refresh period and on DWM/driver behavior. The probe posts window messages, so real HID input adds its own fixed latency on top, the same for every variant.

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
- [x] Lead for a "Frame/render time" session: hardware `EndDraw` takes ~10.7 ms per paint when painting continuously (vs 2–3 ms of drawing). Measure input-to-frame latency while scrolling and typing with `D2D1_PRESENT_OPTIONS_IMMEDIATELY`, or with a flip-model swap chain and a frame-latency waitable object. — see note 2026-10-31 Idle CPU (investigated 2026-11-21; see To Do 6–7)
- [ ] Create the HWND render target with `D2D1_PRESENT_OPTIONS_IMMEDIATELY` (`create_target` in `src/ui/platform/window.rs`), and don't paint again within ~4 ms of the last paint: leave the window dirty and schedule it at the 4 ms mark. Expected impact: continuous wheel scrolling 29 → ~6.5 ms input-to-screen (p95 34 → 12 ms) at 84 Hz. Isolated input and idle CPU stay the same. Wheel/drag frames are capped at ~250/s, versus 85 today and an uncapped 121+ (≈ +15–40% CPU while scrolling). Re-run `benches/frame_latency.ps1` with the patch to confirm. — see note 2026-11-21 Frame time
- [ ] Optional, bigger: replace the HWND target with a flip-model swap chain (D3D11 device, `ID2D1DeviceContext`, `FLIP_DISCARD`, maximum frame latency 1). Wait on its frame-latency waitable in `MsgWaitForMultipleObjectsEx`, and paint only once it signals. Expected impact: clicks and key presses 4–5 → 1.8 ms to screen (p95 12.6 → 2.3 ms) with no wasted frames. Continuous scrolling only improves to 18 ms (A gives 6.5). Costs: swap-chain resize and device-lost handling, and ~+40% CPU while scrolling. Only worth doing together with late-latched painting (start the frame ~3 ms before vblank) to fix the continuous case. The prototype is `DD_FLIP=loop` in `performance/frame-experiments.patch`. — see note 2026-11-21 Frame time