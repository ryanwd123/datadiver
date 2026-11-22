//! Slow, randomized tests that look for crashes and wrong results:
//!
//! - `engines`: the same random table and random searches, filters, sorts
//!   and groupings in DuckDB and Polars give the same rows, and no view
//!   loses or repeats a row.
//! - `inputs`: the search language, condition cards, key strokes, action
//!   templates, pasted values, expression completion, and conditional-format
//!   rules and number formats never panic and keep ranges in their text.
//! - `saved_files`: damaged settings, keymaps, themes, history and views
//!   load as an error or as something usable.
//! - `editor`: random multi-cursor editing commands keep selections on
//!   character boundaries and undo and redo exactly.
//! - `ui::dataframe_view::chaos_tests`: a chaos monkey that drives the whole
//!   viewer, also on profiles whose saved files it damages.
//!
//! They are `#[ignore]`d so `cargo test` stays fast. Run them before a
//! release with `scripts/stress.ps1`, or directly:
//!
//! ```powershell
//! cargo test --lib stress:: -- --ignored
//! ```
//!
//! `DATADIVER_STRESS_CASES` sets how many cases each property test tries,
//! and `DATADIVER_STRESS_SECONDS` how long the chaos monkey runs. A failing
//! property test saves its shrunk input in `proptest-regressions/stress/`,
//! which later runs try first; commit it with the fix.

mod editor;
mod engines;
mod inputs;
pub(crate) mod saved_files;

use proptest::test_runner::Config;

/// How many cases each property test tries: `DATADIVER_STRESS_CASES`, or
/// `default`.
pub(crate) fn cases(default: u32) -> u32 {
    std::env::var("DATADIVER_STRESS_CASES")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

/// A property-test config trying [`cases`] cases.
pub(crate) fn config(default: u32) -> Config {
    Config {
        cases: cases(default),
        // Shrinking re-runs the engines many times; this bounds it.
        max_shrink_iters: 2_000,
        ..Config::default()
    }
}

/// A small, fast, seedable generator for the chaos monkey, whose steps
/// must replay exactly from a seed.
#[derive(Clone, Debug)]
pub(crate) struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// SplitMix64.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n`; `n` must not be zero.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// True with probability `percent` in 100.
    pub fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent as usize
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}
