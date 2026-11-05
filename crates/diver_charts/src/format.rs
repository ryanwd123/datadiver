//! Numbers as charts print them.

use crate::scale::Ticks;

const MINUS: char = '\u{2212}';

/// `v` with thousands separators and exactly `decimals` places.
pub fn number(v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return if v.is_nan() { "NaN".into() } else if v > 0.0 { "∞".into() } else { format!("{MINUS}∞") };
    }
    let text = format!("{:.*}", decimals, v.abs());
    let (whole, frac) = text.split_once('.').map_or((text.as_str(), None), |(w, f)| (w, Some(f)));
    let mut out = String::new();
    // A value that rounds to zero prints without a sign.
    if v < 0.0 && text.bytes().any(|b| b.is_ascii_digit() && b != b'0') {
        out.push(MINUS);
    }
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(frac) = frac {
        out.push('.');
        out.push_str(frac);
    }
    out
}

/// The places needed to print multiples of `step` exactly.
pub fn decimals_for(step: f64) -> usize {
    let mut s = step.abs();
    let mut d = 0;
    while d < 10 && (s - s.round()).abs() > 1e-6 * s.max(1.0) {
        s *= 10.0;
        d += 1;
    }
    d
}

/// `v` with up to `max_decimals` places, trailing zeros dropped.
pub fn trimmed(v: f64, max_decimals: usize) -> String {
    let text = number(v, max_decimals);
    if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text }
}

fn unit_for(magnitude: f64) -> (f64, &'static str) {
    if magnitude >= 1e12 {
        (1e12, "T")
    } else if magnitude >= 1e9 {
        (1e9, "B")
    } else if magnitude >= 1e6 {
        (1e6, "M")
    } else if magnitude >= 1e4 {
        (1e3, "K")
    } else {
        (1.0, "")
    }
}

/// A short form for labels: 1,284 / 12.9K / 4.2M.
pub fn compact(v: f64) -> String {
    let (unit, suffix) = unit_for(v.abs());
    if unit == 1.0 {
        return trimmed(v, 2);
    }
    let scaled = v / unit;
    let decimals = if scaled.abs() >= 100.0 { 0 } else { 1 };
    format!("{}{suffix}", trimmed(scaled, decimals))
}

/// One style for every label on an axis: in full below 10,000, and in
/// thousands, millions or billions on an axis that reaches it.
#[derive(Clone, Copy, Debug)]
pub struct TickFormat {
    unit: f64,
    suffix: &'static str,
    decimals: usize,
}

impl TickFormat {
    pub fn new(ticks: &Ticks) -> TickFormat {
        let magnitude = ticks.min.abs().max(ticks.max.abs());
        let (unit, suffix) = unit_for(magnitude);
        TickFormat { unit, suffix, decimals: decimals_for(ticks.step / unit) }
    }

    pub fn format(&self, v: f64) -> String {
        if v == 0.0 && self.unit > 1.0 {
            return "0".into();
        }
        format!("{}{}", number(v / self.unit, self.decimals), self.suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::nice_ticks;

    #[test]
    fn separators_and_signs() {
        assert_eq!(number(1234567.891, 2), "1,234,567.89");
        assert_eq!(number(-1234.0, 0), "\u{2212}1,234");
        assert_eq!(number(-0.001, 1), "0.0");
        assert_eq!(number(999.0, 0), "999");
    }

    #[test]
    fn compact_forms() {
        assert_eq!(compact(1284.0), "1,284");
        assert_eq!(compact(12_900.0), "12.9K");
        assert_eq!(compact(4_200_000.0), "4.2M");
        assert_eq!(compact(250_000.0), "250K");
        assert_eq!(compact(4.5678), "4.57");
    }

    #[test]
    fn ticks_share_one_style() {
        let t = nice_ticks(0.0, 0.9, 5);
        let f = TickFormat::new(&t);
        assert_eq!(f.format(0.2), "0.2");
        assert_eq!(f.format(0.0), "0.0");
        let t = nice_ticks(0.0, 48_000.0, 5);
        let f = TickFormat::new(&t);
        assert_eq!(f.format(10_000.0), "10K");
        assert_eq!(f.format(0.0), "0");
        let t = nice_ticks(0.0, 8_000.0, 4);
        assert_eq!(TickFormat::new(&t).format(2_000.0), "2,000");
        let t = nice_ticks(0.0, 2_600_000.0, 5);
        assert_eq!(TickFormat::new(&t).format(1_500_000.0), "1.5M");
    }
}
