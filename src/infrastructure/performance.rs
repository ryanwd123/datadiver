//! Opt-in measurements for the real-window benchmark. No logging in normal runs.
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static OUTPUT: OnceLock<PathBuf> = OnceLock::new();
static EVENTS: Mutex<Vec<serde_json::Value>> = Mutex::new(Vec::new());
static SAMPLE: Mutex<Option<(String, usize)>> = Mutex::new(None);

pub fn enabled() -> bool {
    OUTPUT.get().is_some()
}

pub fn sample(scenario: &str, iteration: usize) {
    *SAMPLE.lock().unwrap() = Some((scenario.into(), iteration));
}

pub fn event(value: serde_json::Value) {
    if enabled() {
        EVENTS.lock().unwrap().push(value);
    }
}

/// Captures the benchmark sample at entry, including for asynchronous work.
struct TimedSample {
    name: &'static str,
    start: Instant,
    sample: Option<(String, usize)>,
}

pub struct Span(Option<TimedSample>);

impl Span {
    pub fn new(name: &'static str) -> Self {
        Self(enabled().then(|| TimedSample {
            name,
            start: Instant::now(),
            sample: SAMPLE.lock().unwrap().clone(),
        }))
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(TimedSample {
            name,
            start,
            sample,
        }) = &self.0
        {
            let mut value =
                serde_json::json!({"metric": name, "ms": start.elapsed().as_secs_f64() * 1000.});
            if let Some((scenario, iteration)) = sample {
                value["scenario"] = scenario.clone().into();
                value["iteration"] = (*iteration).into();
            }
            event(value);
        }
    }
}

pub fn frame_signal() -> (
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    (flag.clone(), flag)
}

pub fn enable(path: PathBuf) {
    let _ = OUTPUT.set(path);
}
pub fn record(name: &str, start: Instant) {
    if OUTPUT.get().is_some() {
        let mut value =
            serde_json::json!({"metric": name, "ms": start.elapsed().as_secs_f64() * 1000.});
        if let Some((scenario, iteration)) = &*SAMPLE.lock().unwrap() {
            value["scenario"] = scenario.clone().into();
            value["iteration"] = (*iteration).into();
        }
        event(value);
    }
}
pub fn flush() {
    if let Some(path) = OUTPUT.get() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(
            path,
            serde_json::to_vec_pretty(&*EVENTS.lock().unwrap()).unwrap(),
        ) {
            eprintln!("cannot write performance measurements: {error}");
        }
    }
}
