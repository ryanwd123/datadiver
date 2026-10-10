use std::collections::HashSet;

use super::formatting::{NumericFormat, display_cell};

pub const FALLBACK_WIDTH: f32 = 200.0;
pub const SAMPLE_ROWS: usize = 3000;

pub fn sample_positions(rows: usize) -> Vec<usize> {
    let count = rows.min(SAMPLE_ROWS);
    match count {
        0 => Vec::new(),
        1 => vec![0],
        _ => (0..count)
            .map(|index| ((index as u128 * (rows - 1) as u128) / (count - 1) as u128) as usize)
            .collect(),
    }
}

pub fn candidates(
    values: impl IntoIterator<Item = Option<String>>,
    dtype: &str,
    format: Option<NumericFormat>,
) -> Vec<String> {
    let unique = values
        .into_iter()
        .map(|value| display_cell(value.as_deref(), dtype, format, 200).replace(['\r', '\n'], " "))
        .filter(|text| !text.is_empty())
        .collect::<HashSet<_>>();
    let mut values = unique.into_iter().collect::<Vec<_>>();
    values.sort_by_cached_key(|text| (std::cmp::Reverse(text.chars().count()), text.clone()));
    let mut selected = values.drain(..values.len().min(16)).collect::<Vec<_>>();
    values.sort_by_cached_key(|text| (std::cmp::Reverse(glyph_score(text)), text.clone()));
    selected.extend(values.into_iter().take(16));
    selected
}

fn glyph_score(text: &str) -> usize {
    text.chars()
        .map(|ch| match ch {
            'W' | 'M' | 'w' | 'm' | '@' => 16,
            'i' | 'l' | 'I' | '.' | ',' | '\'' | ' ' => 4,
            '\u{1100}'..='\u{115f}'
            | '\u{2e80}'..='\u{a4cf}'
            | '\u{ac00}'..='\u{d7a3}'
            | '\u{f900}'..='\u{faff}'
            | '\u{ff01}'..='\u{ff60}'
            | '\u{1f300}'..='\u{1faff}' => 20,
            _ => 10,
        })
        .sum()
}

pub fn inferred_width(measured: f32) -> f32 {
    if measured.is_finite() {
        (measured + 24.0).clamp(80.0, 600.0)
    } else {
        FALLBACK_WIDTH
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_is_bounded_even_and_covers_endpoints() {
        assert!(sample_positions(0).is_empty());
        assert_eq!(sample_positions(1), [0]);
        assert_eq!(sample_positions(3), [0, 1, 2]);
        let positions = sample_positions(1_000_000);
        assert_eq!(positions.len(), SAMPLE_ROWS);
        assert_eq!(positions.first(), Some(&0));
        assert_eq!(positions.last(), Some(&999_999));
        assert!(positions.windows(2).all(|pair| pair[1] > pair[0]));
    }

    #[test]
    fn shortlist_is_bounded_and_keeps_wide_unicode_and_formatted_values() {
        let mut values = (20..70).map(|n| Some("i".repeat(n))).collect::<Vec<_>>();
        values.extend([Some("界".repeat(18)), Some("W".repeat(22)), None]);
        let selected = candidates(values, "VARCHAR", None);
        assert_eq!(selected.len(), 32);
        assert!(selected.contains(&"界".repeat(18)));
        assert!(selected.contains(&"W".repeat(22)));
        assert_eq!(
            candidates(
                [Some("1000".into()), Some("1000".into())],
                "BIGINT",
                Some(NumericFormat {
                    precision: 0,
                    grouping: true
                })
            ),
            ["1,000"]
        );
        assert!(candidates([None], "VARCHAR", None).is_empty());
        assert_eq!(inferred_width(0.), 80.);
        assert_eq!(inferred_width(1000.), 600.);
        assert_eq!(inferred_width(f32::NAN), 200.);
    }
}
