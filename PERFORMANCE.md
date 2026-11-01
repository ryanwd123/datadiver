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
| _(none yet)_ | | | | |

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

_No entries yet._

---

## To Do

<!--
Format: - [ ] <Improvement> — expected impact: <e.g. ~40% faster file open> — see note YYYY-MM-DD <Area>
When done: - [x] ... (done YYYY-MM-DD, commit abc1234)
Keep completed items as history; don't delete them.
-->

_No items yet._