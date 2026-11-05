//! Scales: values to positions, and the round tick values an axis shows.

/// Round tick values covering a range.
#[derive(Clone, Debug, PartialEq)]
pub struct Ticks {
    /// The axis runs from `min` to `max`, both multiples of `step`.
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub values: Vec<f64>,
}

/// A step of 1, 2 or 5 times a power of ten that cuts `span` into about
/// `count` pieces.
pub fn nice_step(span: f64, count: usize) -> f64 {
    let raw = span.abs() / count.max(1) as f64;
    if raw <= 0.0 || !raw.is_finite() {
        return 1.0;
    }
    let magnitude = 10f64.powf(raw.log10().floor());
    let norm = raw / magnitude;
    let nice = if norm < 1.5 {
        1.0
    } else if norm < 3.0 {
        2.0
    } else if norm < 7.0 {
        5.0
    } else {
        10.0
    };
    nice * magnitude
}

/// Ticks from at or below `lo` to at or above `hi`, about `count` steps apart.
pub fn nice_ticks(lo: f64, hi: f64, count: usize) -> Ticks {
    let (mut lo, mut hi) = if lo.is_finite() && hi.is_finite() { (lo.min(hi), lo.max(hi)) } else { (0.0, 1.0) };
    if lo == hi {
        // One value: give it room on the side away from zero.
        let pad = if lo == 0.0 { 1.0 } else { lo.abs() * 0.5 };
        if lo >= 0.0 { hi += pad } else { lo -= pad }
    }
    let step = nice_step(hi - lo, count);
    // Whole numbers of steps keep the values exact multiples of the step.
    let first = (lo / step + 1e-9).floor() as i64;
    let last = (hi / step - 1e-9).ceil() as i64;
    let values = (first..=last).map(|k| k as f64 * step).collect();
    Ticks { min: first as f64 * step, max: last as f64 * step, step, values }
}

/// Maps values in `[d0, d1]` onto positions in `[r0, r1]`.
#[derive(Clone, Copy, Debug)]
pub struct Linear {
    pub d0: f64,
    pub d1: f64,
    pub r0: f32,
    pub r1: f32,
}

impl Linear {
    pub fn new((d0, d1): (f64, f64), (r0, r1): (f32, f32)) -> Linear {
        Linear { d0, d1, r0, r1 }
    }

    pub fn map(&self, v: f64) -> f32 {
        let span = self.d1 - self.d0;
        let t = if span == 0.0 { 0.0 } else { (v - self.d0) / span };
        self.r0 + (self.r1 - self.r0) * t as f32
    }
}

/// Splits a length into `n` equal bands with gaps between and around them,
/// as fractions of one step.
#[derive(Clone, Copy, Debug)]
pub struct Band {
    pub n: usize,
    pub start: f32,
    pub step: f32,
    pub width: f32,
    outer: f32,
}

impl Band {
    pub fn new(n: usize, (start, end): (f32, f32), inner: f32, outer: f32) -> Band {
        let n_f = n.max(1) as f32;
        let step = (end - start) / (n_f - inner + 2.0 * outer).max(1e-6);
        Band { n, start, step, width: step * (1.0 - inner), outer }
    }

    /// Where band `i` starts.
    pub fn at(&self, i: usize) -> f32 {
        self.start + (self.outer + i as f32) * self.step
    }

    pub fn center(&self, i: usize) -> f32 {
        self.at(i) + self.width / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_one_two_or_five() {
        assert_eq!(nice_step(100.0, 5), 20.0);
        assert_eq!(nice_step(1.0, 10), 0.1);
        assert_eq!(nice_step(730.0, 5), 100.0);
        assert_eq!(nice_step(900.0, 5), 200.0);
        assert_eq!(nice_step(0.034, 4), 0.01);
    }

    #[test]
    fn ticks_cover_the_range() {
        let t = nice_ticks(0.0, 87.0, 5);
        assert_eq!((t.min, t.max, t.step), (0.0, 100.0, 20.0));
        assert_eq!(t.values, vec![0.0, 20.0, 40.0, 60.0, 80.0, 100.0]);
        let t = nice_ticks(-12.0, 31.0, 4);
        assert_eq!((t.min, t.max, t.step), (-20.0, 40.0, 10.0));
    }

    #[test]
    fn a_tick_on_the_edge_needs_no_extra_step() {
        let t = nice_ticks(0.0, 100.0, 5);
        assert_eq!(t.max, 100.0);
    }

    #[test]
    fn one_value_still_gets_an_axis() {
        let t = nice_ticks(0.0, 0.0, 5);
        assert!(t.max > 0.0);
        let t = nice_ticks(-4.0, -4.0, 5);
        assert!(t.min < -4.0 && t.max >= -4.0);
    }

    #[test]
    fn bands_fill_the_length() {
        let b = Band::new(4, (0.0, 100.0), 0.2, 0.1);
        let end = b.at(3) + b.width + b.step * 0.1;
        assert!((end - 100.0).abs() < 1e-3);
        assert!(b.at(1) > b.at(0) + b.width);
    }
}
