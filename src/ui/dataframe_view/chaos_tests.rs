//! A chaos monkey: drives the whole viewer with random key presses, typing,
//! clicks on whatever the last frame drew, drags, wheel turns, pastes,
//! dropped files, resizes, waits and restarts, and checks after every step
//! that nothing panicked, every window still paints, and the grid's state
//! holds together.
//!
//! The steps come from a seed, so a failure replays exactly. When one
//! fails, the monkey shrinks the steps to a short list that still fails
//! and prints it with the command that replays the seed.
//!
//! ```powershell
//! # Run for two minutes from a random seed.
//! $env:DATADIVER_STRESS_SECONDS = 120
//! cargo test --lib -- --ignored --exact ui::dataframe_view::chaos_tests::stress_chaos --nocapture
//! # Replay one seed.
//! $env:DATADIVER_CHAOS_SEED = 1234
//! cargo test --lib -- --ignored --exact ui::dataframe_view::chaos_tests::stress_chaos --nocapture
//! ```

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::FileEntry;
use super::test_support::{Viewer, empty_frame, repository};
use crate::application::keymap::ACTIONS;
use crate::application::state::ApplicationState;
use crate::stress::Rng;
use crate::ui::platform::system::{ClipboardContent, fake};
use crate::ui::platform::{Event, Modifiers, Point, WindowId, point};

/// Keys worth pressing anywhere, besides every default binding.
const KEYS: &[&str] = &[
    "escape", "escape", "enter", "tab", "shift-tab", "up", "down", "left", "right", "shift-down", "shift-right",
    "ctrl-shift-right", "home", "end", "ctrl-home", "ctrl-end", "pageup", "pagedown", "backspace", "delete",
    "ctrl-a", "ctrl-c", "ctrl-v", "ctrl-x", "ctrl-z", "ctrl-y", "ctrl-shift-z", "space", "f1", "f2", "f5",
    "alt-up", "alt-down", "ctrl-backspace", "ctrl-delete", "a", "s", "1", "-", "/", "?", ":", "\"",
    "ctrl-space", "shift-space", "ctrl-shift-p", "ctrl-p", "ctrl-k", "ctrl-f", "ctrl-g", "ctrl-w", "ctrl-n",
];

/// Text to type: searches, conditions, SQL, format rules, names, and
/// awkward characters.
const TEXT: &[&str] = &[
    "north", "amount>5", "-x", "region:no | south", "\"amount\">\"price\"", "day>2024-01", "price<=0.5",
    "1..10", ":or", "!", "null", "2024-02-29", "日本", "🙂", "Straße", "\"", "'", "\\", " ", "\n", "\t",
    "select * from data", "select region, sum(amount) from data group by 1",
    "with x as (select 1 as a) select * from x", "select * from read_csv('missing.csv')", "select 1/0",
    "select [1, 2, 3] as l, {'a': 1} as s", "select * from range(5000)", "select repeat('x', 5000) as long",
    "select * from data where", "select \"amount\", from", ")", "(((", "--", "/*", "';", "sum(amount)",
    "count(*)", "upper(region)", "case when amount > 1 then 'a' end", "if c > 0 then bg: red",
    "bar(c, min: 0, max: 100)", "scale(c, #f8696b, #63be7b)", "if c contains 'x' then bold", "#ff00",
    "Sales by region", "a", "zz", "0", "-1", "1e308", "99999999999999999999999", "3.14159",
];

const SIZES: &[(f32, f32)] = &[(900.0, 600.0), (1920.0, 1080.0), (320.0, 240.0), (120.0, 80.0), (1.0, 1.0), (2600.0, 300.0), (300.0, 1600.0)];

/// One input, with what it needs to pick its target from the screen when it
/// runs, so a shorter list of steps still makes sense.
#[derive(Clone)]
enum Step {
    Press(String),
    Type(String),
    Click { window: u64, pick: u64, clicks: u32, modifiers: u8 },
    RightClick { window: u64, pick: u64 },
    Drag { window: u64, from: u64, to: u64 },
    Hover { window: u64, pick: u64 },
    Wheel { window: u64, pick: u64, delta: (f32, f32) },
    Paste(String),
    PastePaths(Vec<usize>),
    Resize(f32, f32),
    Wait(u64),
    Drop(Vec<usize>),
    /// Answers the next file dialogs with fixture files, or with a new file
    /// in the profile for save dialogs.
    AnswerDialogs(usize),
    Restart,
    /// Closes the app, damages some of the files it saved, and reopens it.
    DamageAndRestart(u64),
}

impl fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Press(keys) => write!(f, "press {keys}"),
            Self::Type(text) => write!(f, "type {text:?}"),
            Self::Click { window, pick, clicks, modifiers } => {
                write!(f, "click window#{window} region#{pick} ×{clicks} modifiers {modifiers:#b}")
            }
            Self::RightClick { window, pick } => write!(f, "right-click window#{window} region#{pick}"),
            Self::Drag { window, from, to } => write!(f, "drag window#{window} region#{from} → region#{to}"),
            Self::Hover { window, pick } => write!(f, "hover window#{window} region#{pick}"),
            Self::Wheel { window, pick, delta } => write!(f, "wheel window#{window} region#{pick} by {delta:?}"),
            Self::Paste(text) => write!(f, "paste {text:?}"),
            Self::PastePaths(files) => write!(f, "paste files {files:?}"),
            Self::Resize(width, height) => write!(f, "resize to {width} × {height}"),
            Self::Wait(ms) => write!(f, "wait {ms} ms"),
            Self::Drop(files) => write!(f, "drop files {files:?}"),
            Self::AnswerDialogs(file) => write!(f, "answer dialogs with file {file}"),
            Self::Restart => write!(f, "close and reopen"),
            Self::DamageAndRestart(seed) => write!(f, "close, damage saved files with seed {seed}, and reopen"),
        }
    }
}

fn bindings() -> Vec<&'static str> {
    let mut keys: Vec<&str> = ACTIONS.iter().flat_map(|action| action.default_bindings.iter().copied()).collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

fn steps(seed: u64, count: usize) -> Vec<Step> {
    let mut rng = Rng::new(seed);
    let bindings = bindings();
    (0..count)
        .map(|_| {
            let window = rng.next_u64();
            let pick = rng.next_u64();
            match rng.below(100) {
                0..=24 => Step::Press((*rng.pick(&bindings)).to_owned()),
                25..=39 => Step::Press((*rng.pick(KEYS)).to_owned()),
                40..=51 => {
                    let mut text = (*rng.pick(TEXT)).to_owned();
                    if rng.chance(10) {
                        text = text.repeat(1 + rng.below(40));
                    }
                    Step::Type(text)
                }
                52..=71 => Step::Click {
                    window,
                    pick,
                    clicks: if rng.chance(15) { 2 } else { 1 },
                    modifiers: if rng.chance(15) { rng.below(4) as u8 } else { 0 },
                },
                72..=74 => Step::RightClick { window, pick },
                75..=78 => Step::Drag { window, from: pick, to: rng.next_u64() },
                79..=80 => Step::Hover { window, pick },
                81..=83 => {
                    let lines = [-30.0, -3.0, -1.0, 1.0, 3.0, 30.0, 1e6];
                    Step::Wheel { window, pick, delta: (if rng.chance(20) { *rng.pick(&lines) } else { 0.0 }, *rng.pick(&lines)) }
                }
                84..=86 => {
                    let tables = ["a\tb\n1\t2\n", "North\nSouth\n\"East\"\n", "x", "", "1\t\t\n\t2", "日本\r\n🙂"];
                    if rng.chance(70) {
                        Step::Paste((*rng.pick(&tables)).to_owned())
                    } else {
                        Step::Paste((*rng.pick(TEXT)).to_owned())
                    }
                }
                87 => Step::PastePaths(vec![rng.below(FILES.len())]),
                88..=89 => {
                    let (width, height) = *rng.pick(SIZES);
                    Step::Resize(width, height)
                }
                90..=94 => Step::Wait(*rng.pick(&[16, 100, 300, 600, 1_000, 2_500, 10_000])),
                95..=96 => Step::Drop((0..1 + rng.below(2)).map(|_| rng.below(FILES.len())).collect()),
                97..=98 => Step::AnswerDialogs(rng.below(FILES.len())),
                _ if rng.chance(50) => Step::DamageAndRestart(rng.next_u64()),
                _ => Step::Restart,
            }
        })
        .collect()
}

/// The fixture files, made fresh in each run's profile.
const FILES: &[&str] = &["data.csv", "nested.parquet", "wide.csv", "empty.csv", "broken.csv", "rows.json", "missing.csv", "folder"];

fn make_fixtures(home: &Path) {
    let mut csv = String::from("region,amount,price,day,stamp,flag,note\n");
    let regions = ["North", "South", "East", "", "Nörth", "a,b", "日本"];
    for n in 0..300i64 {
        let region = regions[n as usize % regions.len()];
        let region = if region.contains(',') { format!("\"{region}\"") } else { region.to_owned() };
        let amount = if n % 11 == 0 { String::new() } else { ((n * 37) % 101 - 50).to_string() };
        let price = format!("{:.3}", (n as f64 * 1.37).sin() * 1000.0);
        let day = format!("2024-{:02}-{:02}", 1 + n % 12, 1 + n % 28);
        let stamp = format!("2024-03-{:02} {:02}:{:02}:00", 1 + n % 28, n % 24, n % 60);
        let note = if n % 7 == 0 { "\"multi\nline\"".to_owned() } else { "x".repeat((n % 5) as usize * 30) };
        csv.push_str(&format!("{region},{amount},{price},{day},{stamp},{},{note}\n", n % 3 == 0));
    }
    std::fs::write(home.join("data.csv"), csv).unwrap();
    let wide = (0..60).map(|c| format!("c{c}")).collect::<Vec<_>>().join(",");
    let row = (0..60).map(|c| c.to_string()).collect::<Vec<_>>().join(",");
    std::fs::write(home.join("wide.csv"), format!("{wide}\n{row}\n{row}\n")).unwrap();
    std::fs::write(home.join("empty.csv"), "a,b\n").unwrap();
    std::fs::write(home.join("broken.csv"), [0xff, 0xfe, 0x00, b'"', b',', b'\n', 0x80, b'"']).unwrap();
    std::fs::write(home.join("rows.json"), r#"[{"a": 1, "b": [1, 2]}, {"a": null, "b": []}, {"c": {"d": "x"}}]"#).unwrap();
    std::fs::create_dir_all(home.join("folder")).unwrap();
    let connection = duckdb::Connection::open_in_memory().unwrap();
    let parquet = home.join("nested.parquet").display().to_string().replace('\'', "''");
    connection
        .execute_batch(&format!(
            "COPY (SELECT range AS id, [range, range + 1] AS list, {{'a': range, 'b': 'x' || range}} AS s, \
             MAP {{'k': range}} AS m, range::DECIMAL(18, 3) / 7 AS d, INTERVAL (range) HOUR AS i, \
             uuid() AS u, NULL::VARCHAR AS nothing, 'b'::BLOB AS bytes, TIMESTAMPTZ '2024-01-01 00:00:00+00' AS tz \
             FROM range(50)) TO '{parquet}' (FORMAT PARQUET)"
        ))
        .unwrap();
}

/// A run's viewer, on a profile in a temporary directory.
struct Run {
    home: tempfile::TempDir,
    viewer: Option<Viewer>,
}

impl Run {
    fn new(seed: u64) -> Self {
        let home = tempfile::tempdir().unwrap();
        make_fixtures(home.path());
        fake::with(|system| *system = Default::default());
        let start = ["data.csv", "data.csv", "nested.parquet", "wide.csv", "empty.csv"][seed as usize % 5];
        let viewer = Viewer::file(&home.path().join(start));
        Self { home, viewer: Some(viewer) }
    }

    fn viewer(&self) -> &Viewer {
        self.viewer.as_ref().unwrap()
    }

    fn file(&self, index: usize) -> PathBuf {
        self.home.path().join(FILES[index])
    }

    /// The window a step acts on: mostly the one taking input, sometimes
    /// another open window.
    fn window(&self, pick: u64) -> WindowId {
        let viewer = self.viewer();
        let windows = viewer.app.windows();
        if pick % 10 < 7 || windows.len() < 2 {
            viewer.active()
        } else {
            windows[pick as usize / 10 % windows.len()]
        }
    }

    /// A point on a region the window drew last, or anywhere in it when it
    /// drew none.
    fn target(&self, window: WindowId, pick: u64) -> Point {
        let regions = self.viewer().app.hits(window);
        if regions.is_empty() || pick % 20 == 0 {
            let x = (pick >> 8) % 1000;
            let y = (pick >> 24) % 700;
            return point(x as f32, y as f32);
        }
        let rect = regions[pick as usize % regions.len()].rect;
        // Mostly the middle; sometimes an edge, where resize handles live.
        match (pick >> 40) % 4 {
            0 => point(rect.x + 1.0, rect.y + rect.h / 2.0),
            1 => point(rect.x + rect.w - 1.0, rect.y + rect.h / 2.0),
            _ => rect.center(),
        }
    }

    fn step(&mut self, step: &Step) {
        let viewer = self.viewer().clone();
        let app = &viewer.app;
        match step {
            Step::Press(keys) => viewer.press(keys),
            Step::Type(text) => viewer.input(text),
            Step::Click { window, pick, clicks, modifiers } => {
                let window = self.window(*window);
                app.render(window);
                let at = self.target(window, *pick);
                let modifiers = Modifiers {
                    shift: modifiers & 1 != 0,
                    control: modifiers & 2 != 0,
                    ..Modifiers::NONE
                };
                for count in 1..=*clicks {
                    if app.windows().contains(&window) {
                        app.mouse_down(window, at, modifiers, count);
                    }
                    if app.windows().contains(&window) {
                        app.mouse_up(window, at);
                    }
                }
            }
            Step::RightClick { window, pick } => {
                let window = self.window(*window);
                app.render(window);
                let at = self.target(window, *pick);
                app.right_click(window, at);
            }
            Step::Drag { window, from, to } => {
                let window = self.window(*window);
                app.render(window);
                let (from, to) = (self.target(window, *from), self.target(window, *to));
                app.mouse_down(window, from, Modifiers::NONE, 1);
                for i in 1..=4 {
                    let t = i as f32 / 4.0;
                    if app.windows().contains(&window) {
                        app.mouse_move(window, point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t), true);
                    }
                }
                if app.windows().contains(&window) {
                    app.mouse_up(window, to);
                }
            }
            Step::Hover { window, pick } => {
                let window = self.window(*window);
                app.render(window);
                let at = self.target(window, *pick);
                app.mouse_move(window, at, false);
                viewer.advance_clock(Duration::from_millis(800));
            }
            Step::Wheel { window, pick, delta } => {
                let window = self.window(*window);
                app.render(window);
                let at = self.target(window, *pick);
                app.scroll(window, at, point(delta.0, delta.1));
            }
            Step::Paste(text) => {
                fake::write_clipboard(text);
                viewer.press("ctrl-v");
            }
            Step::PastePaths(files) => {
                let paths = files.iter().map(|file| self.file(*file)).collect();
                fake::with(|system| system.clipboard = Some(ClipboardContent::Paths(paths)));
                viewer.press("ctrl-v");
            }
            Step::Resize(width, height) => app.resize(viewer.active(), *width, *height),
            Step::Wait(ms) => viewer.advance_clock(Duration::from_millis(*ms)),
            Step::Drop(files) => {
                let paths = files.iter().map(|file| self.file(*file)).collect();
                let window = viewer.active();
                app.dispatch(window, Event::Drop { paths, pos: point(450.0, 300.0), hit: None });
            }
            Step::AnswerDialogs(file) => {
                let path = self.file(*file);
                let save = self.home.path().join(["export.csv", "export.xlsx", "export.parquet", "export.json", "out.sql"][*file % 5]);
                fake::with(|system| {
                    system.path_answers.push_back(Some(vec![path]));
                    system.new_path_answers.push_back(Some(save));
                });
            }
            Step::Restart => self.restart(),
            Step::DamageAndRestart(seed) => {
                self.close();
                let mut rng = Rng::new(*seed);
                let files = crate::stress::saved_files::profile_files(self.home.path());
                for _ in 0..1 + rng.below(3) {
                    if files.is_empty() {
                        break;
                    }
                    let file = rng.pick(&files);
                    let Ok(bytes) = std::fs::read(file) else { continue };
                    match crate::stress::saved_files::damage(&bytes, &mut rng) {
                        Some(damaged) => std::fs::write(file, damaged).unwrap(),
                        None => std::fs::remove_file(file).unwrap(),
                    }
                }
                self.restart();
            }
        }
    }

    fn close(&mut self) {
        if let Some(viewer) = self.viewer.take() {
            if viewer.is_open() {
                viewer.request_close();
            }
            drop(viewer);
        }
    }

    /// Closes every window and starts again on the same profile, restoring
    /// the last view like the app does at startup.
    fn restart(&mut self) {
        self.close();
        let repository = repository(self.home.path());
        let entry = match ApplicationState::load(repository.clone()) {
            Ok(state) => state
                .history
                .last_view_id
                .as_ref()
                .and_then(|id| state.views.iter().find(|view| &view.id == id))
                .cloned()
                .map(FileEntry::View)
                .or_else(|| state.history.last_raw_source.map(|path| FileEntry::Source(path.into()))),
            Err(_) => None,
        }
        .unwrap_or_else(|| FileEntry::Source(self.home.path().join("data.csv")));
        let viewer = Viewer::open(empty_frame(), None, None, repository);
        viewer.update(|view, cx| view.navigate_to_file(entry, false, false, false, cx));
        viewer.run_until_parked();
        self.viewer = Some(viewer);
    }

    /// After a step: the work it started finishes, every window paints, and
    /// the state holds together. A closed main window starts a new one.
    fn settle_and_check(&mut self) {
        if !self.viewer().is_open() {
            self.restart();
        }
        let viewer = self.viewer().clone();
        viewer.run_until_parked();
        for window in viewer.app.windows() {
            viewer.app.render(window);
        }
        viewer.read(|view| {
            if let Err(error) = view.grid.check_invariants() {
                panic!("grid invariant broken: {error}");
            }
            SURFACES.with(|seen| {
                let mut seen = seen.borrow_mut();
                for surface in surfaces(view) {
                    *seen.entry(surface).or_default() += 1;
                }
            });
        });
    }
}

thread_local! {
    /// How many steps ended on each dialog and window, to show what the
    /// monkey reached.
    static SURFACES: std::cell::RefCell<std::collections::BTreeMap<String, usize>> = Default::default();
}

/// The dialog and secondary windows open on `view`.
fn surfaces(view: &super::DataFrameView) -> Vec<String> {
    use super::Dialog;
    let mut open = Vec::new();
    if let Some(dialog) = &view.dialog {
        open.push(match dialog {
            Dialog::Save { .. } => "dialog: save".to_owned(),
            Dialog::Confirm(_) => "dialog: confirm".to_owned(),
            Dialog::Information(_) => "dialog: information".to_owned(),
            Dialog::Pivot(_) => "dialog: pivot".to_owned(),
            Dialog::OrderBy(_) => "dialog: order by".to_owned(),
            Dialog::Having(_) => "dialog: having".to_owned(),
            Dialog::Aggregation(_) => "dialog: aggregation".to_owned(),
            Dialog::Palette(palette) => format!("palette: {}", palette.title),
            Dialog::SlicerPicker(_) => "dialog: slicer picker".to_owned(),
            Dialog::Theme(_) => "dialog: theme".to_owned(),
            Dialog::Actions(_) => "dialog: actions".to_owned(),
            Dialog::AiSettings(_) => "dialog: AI settings".to_owned(),
            Dialog::PasteFilter(_) => "dialog: paste filter".to_owned(),
            Dialog::ResultFilter { .. } => "dialog: result filter".to_owned(),
        });
    }
    for (name, is_open) in [
        ("window: formats", view.format_window.is_some()),
        ("window: panel", view.panel_window.is_some()),
        ("window: help", view.help_window.is_some()),
        ("window: SQL", view.sql_window.is_some()),
    ] {
        if is_open {
            open.push(name.to_owned());
        }
    }
    if open.is_empty() {
        open.push("grid only".to_owned());
    }
    open
}

/// Runs `steps` on a fresh profile; `Err` holds the panic message and the
/// index of the failing step.
fn run(seed: u64, steps: &[Step]) -> Result<(), (usize, String)> {
    let mut run = None;
    let mut index = 0;
    let result = catch_unwind(AssertUnwindSafe(|| {
        crate::infrastructure::launcher::tests::BLOCKED.with(|blocked| blocked.set(true));
        let run = run.insert(Run::new(seed));
        for step in steps {
            run.step(step);
            run.settle_and_check();
            index += 1;
        }
    }));
    match result {
        Ok(()) => Ok(()),
        Err(payload) => {
            // The views may be half-changed; closing them could panic again
            // and abort, so the run is leaked instead.
            if let Some(run) = run.take() {
                std::mem::forget(run.viewer);
            }
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "panic".into());
            Err((index, message))
        }
    }
}

/// The first line of a panic message, which a shrunk run must reproduce.
fn signature(message: &str) -> &str {
    message.lines().next().unwrap_or_default()
}

/// Removes steps while the run still fails the same way: first halves,
/// then smaller chunks, down to single steps.
fn shrink(seed: u64, mut steps: Vec<Step>, message: &str, budget: Duration) -> Vec<Step> {
    let wanted = signature(message).to_owned();
    let started = Instant::now();
    let fails = |steps: &[Step]| matches!(run(seed, steps), Err((_, m)) if signature(&m) == wanted);
    let mut chunk = steps.len().div_ceil(2).max(1);
    while chunk >= 1 && started.elapsed() < budget {
        let mut start = 0;
        let mut removed = false;
        while start < steps.len() && started.elapsed() < budget {
            let mut candidate = steps.clone();
            candidate.drain(start..(start + chunk).min(candidate.len()));
            if fails(&candidate) {
                steps = candidate;
                removed = true;
            } else {
                start += chunk;
            }
        }
        if !removed {
            if chunk == 1 {
                break;
            }
            chunk = chunk.div_ceil(2);
        }
    }
    steps
}

/// Where the last caught panic happened, and its backtrace.
static PANIC_SITE: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);

fn env<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok().and_then(|value| value.trim().parse().ok())
}

#[test]
#[ignore = "stress test: run with scripts/stress.ps1"]
fn stress_chaos() {
    drive("stress_chaos", steps);
}

/// Chaos whose profile is damaged every twenty steps, then reopened.
#[test]
#[ignore = "stress test: run with scripts/stress.ps1"]
fn stress_chaos_damaged_profile() {
    drive("stress_chaos_damaged_profile", |seed, count| {
        let mut rng = Rng::new(!seed);
        let mut all = Vec::new();
        for (index, step) in steps(seed, count).into_iter().enumerate() {
            all.push(step);
            if index % 20 == 19 {
                all.push(Step::DamageAndRestart(rng.next_u64()));
            }
        }
        all
    });
}

/// Runs the steps `make` gives for seed after seed until the time budget is
/// spent, shrinking and reporting the first failure.
fn drive(test: &str, make: impl Fn(u64, usize) -> Vec<Step>) {
    let budget = Duration::from_secs(env("DATADIVER_STRESS_SECONDS").unwrap_or(60));
    let count = env("DATADIVER_CHAOS_STEPS").unwrap_or(300);
    let fixed = env::<u64>("DATADIVER_CHAOS_SEED");
    let first = fixed.unwrap_or_else(|| {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64
    });
    let started = Instant::now();
    let mut runs = 0u64;
    loop {
        let seed = first.wrapping_add(runs);
        let steps = make(seed, count);
        // Panics are caught and reported below; the default hook would
        // print each one again while shrinking. This one keeps where the
        // last panic happened, since the message alone does not say.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|info| {
            let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            let trace = std::backtrace::Backtrace::force_capture().to_string();
            *PANIC_SITE.lock().unwrap() = Some((at, trace));
        }));
        let result = run(seed, &steps);
        if let Err((index, message)) = result {
            eprintln!("chaos: seed {seed} failed at step {index} of {count}: {message}");
            let shrunk = shrink(seed, steps[..=index.min(steps.len() - 1)].to_vec(), &message, Duration::from_secs(300));
            // Where the shortest run panics, with the frames of this workspace.
            let _ = run(seed, &shrunk);
            std::panic::set_hook(hook);
            let (at, trace) = PANIC_SITE.lock().unwrap().take().unwrap_or_default();
            let trace = trace
                .lines()
                .filter(|line| ["datadiver", "diver_", "code_editor"].iter().any(|name| line.contains(name)))
                .take(30)
                .collect::<Vec<_>>()
                .join("\n");
            let listed = shrunk.iter().map(|step| format!("    {step:?}")).collect::<Vec<_>>().join("\n");
            panic!(
                "chaos seed {seed} failed: {message}\n  at {at}\n{trace}\n\nShortest failing steps found ({} of {}):\n{listed}\n\n\
                 Replay with: $env:DATADIVER_CHAOS_SEED = {seed}; $env:DATADIVER_CHAOS_STEPS = {}; \
                 cargo test --lib -- --ignored --exact ui::dataframe_view::chaos_tests::{test} --nocapture",
                shrunk.len(),
                index + 1,
                index + 1,
            );
        }
        std::panic::set_hook(hook);
        runs += 1;
        if fixed.is_some() || started.elapsed() >= budget {
            break;
        }
    }
    eprintln!("chaos: {runs} runs of {count} steps passed from seed {first} in {:.0?}", started.elapsed());
    eprintln!("chaos: steps that ended on each surface:");
    SURFACES.with(|seen| {
        for (surface, steps) in seen.borrow().iter() {
            eprintln!("  {steps:>6}  {surface}");
        }
    });
}

#[test]
fn chaos_steps_replay_from_their_seed() {
    let (a, b) = (steps(7, 50), steps(7, 50));
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
    assert_ne!(format!("{a:?}"), format!("{:?}", steps(8, 50)));
}
