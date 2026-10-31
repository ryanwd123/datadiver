//! The formatter over the curated corpus: it changes only whitespace, and
//! formatting again changes nothing.

use std::path::Path;

use diver_sql::{EditStyle, SyntaxKind, format_sql, tokenize};

/// The tokens of `text` other than whitespace, as text.
fn words(text: &str) -> Vec<String> {
    let mut at = 0;
    tokenize(text)
        .into_iter()
        .filter_map(|t| {
            let start = at;
            at += usize::from(t.len);
            (t.kind != SyntaxKind::WHITESPACE).then(|| text[start..at].to_string())
        })
        .collect()
}

#[test]
fn formatting_keeps_tokens_and_is_stable() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let style = EditStyle::default();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "sql") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(once) = format_sql(line, None, &style) else {
                continue;
            };
            assert_eq!(words(&once.text), words(line), "tokens changed:\n{line}\n---\n{}", once.text);
            let twice = format_sql(&once.text, None, &style).unwrap();
            assert_eq!(twice.text, once.text, "not stable:\n{line}");
        }
    }
}
