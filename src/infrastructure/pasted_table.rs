//! Saves pasted text as a CSV or TSV file so it opens like any other source:
//! it gets a history entry, can be reloaded, and views can be saved over it.

use std::path::{Path, PathBuf};

/// Writes `text` under `directory` and returns the new file's path.
///
/// Tab-separated text (as copied from a spreadsheet or the grid) becomes a
/// `.tsv`; text whose first line has commas and no tabs becomes a `.csv`.
/// The first line is the header, as for any CSV or TSV source.
pub fn save(directory: &Path, text: &str, stamp: &str) -> Result<PathBuf, String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Err("The clipboard has no text to open as a table".into());
    }
    let extension = extension(text);
    std::fs::create_dir_all(directory).map_err(|error| {
        format!(
            "Cannot create pasted table folder {}: {error}",
            directory.display()
        )
    })?;
    let mut path = directory.join(format!("pasted {stamp}.{extension}"));
    let mut suffix = 2;
    while path.exists() {
        path = directory.join(format!("pasted {stamp} ({suffix}).{extension}"));
        suffix += 1;
    }
    std::fs::write(&path, text)
        .map_err(|error| format!("Cannot save pasted table {}: {error}", path.display()))?;
    Ok(path)
}

fn extension(text: &str) -> &'static str {
    let header = text.lines().next().unwrap_or("");
    if !header.contains('\t') && header.contains(',') {
        "csv"
    } else {
        "tsv"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_text_saves_as_tsv_and_comma_text_as_csv() {
        let directory = tempfile::tempdir().unwrap();
        let tsv = save(directory.path(), "a\tb\r\n1\t2\r\n", "1").unwrap();
        assert_eq!(tsv.extension().unwrap(), "tsv");
        assert_eq!(std::fs::read_to_string(&tsv).unwrap(), "a\tb\r\n1\t2\r\n");
        let csv = save(directory.path(), "a,b\n1,\"x\ty\"\n", "2").unwrap();
        assert_eq!(csv.extension().unwrap(), "csv");
        // A single column has no separator to detect, and reads as TSV.
        let single = save(directory.path(), "name\nAda\n", "3").unwrap();
        assert_eq!(single.extension().unwrap(), "tsv");
    }

    #[test]
    fn repeated_stamps_do_not_overwrite_and_blank_text_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let first = save(directory.path(), "a\n1\n", "same").unwrap();
        let second = save(directory.path(), "b\n2\n", "same").unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(first).unwrap(), "a\n1\n");
        assert!(save(directory.path(), " \r\n ", "x").is_err());
    }
}
