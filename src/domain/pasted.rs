//! Values pasted from the clipboard to filter by, such as a column copied
//! from Excel.

/// The distinct values in `text`, in order: one per line, and one per cell
/// when the copied cells span several columns. Values are trimmed, blank
/// ones are skipped, and a cell Excel quoted is unquoted.
pub fn pasted_values(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    text.split(['\n', '\t'])
        .map(|value| unquote(value.trim()))
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .collect()
}

/// Excel quotes a cell holding a quote, and doubles the quotes inside it.
fn unquote(value: &str) -> String {
    match value.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => value.to_owned(),
    }
}

/// A short list of `values` for a preview: the first few, then a count of
/// the rest.
pub fn preview(values: &[String], shown: usize) -> String {
    let mut listed: Vec<String> = values
        .iter()
        .take(shown)
        .map(|value| crate::domain::formatting::truncate(value, 24))
        .collect();
    if values.len() > shown {
        listed.push(format!("+{} more", values.len() - shown));
    }
    listed.join(", ")
}

#[cfg(test)]
mod tests {
    use super::{pasted_values, preview};

    #[test]
    fn pasted_values_split_lines_and_cells_and_skip_blanks_and_repeats() {
        assert_eq!(
            pasted_values("North\r\nSouth\r\n\r\n  East \r\nNorth\r\n"),
            ["North", "South", "East"]
        );
        assert_eq!(pasted_values("a\tb\r\nc\t\r\n"), ["a", "b", "c"]);
        assert_eq!(pasted_values("\"say \"\"hi\"\"\"\n\"\"\n"), ["say \"hi\""]);
        assert!(pasted_values(" \r\n\t").is_empty());
    }

    #[test]
    fn preview_lists_the_first_values_and_counts_the_rest() {
        let values: Vec<String> = ["a", "b", "c", "d"].map(String::from).into();
        assert_eq!(preview(&values, 3), "a, b, c, +1 more");
        assert_eq!(preview(&values[..2], 3), "a, b");
    }
}
