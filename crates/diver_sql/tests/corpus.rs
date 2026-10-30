//! The curated corpus: statements DuckDB accepts that must parse without errors.

use std::path::Path;

#[test]
fn curated_corpus_parses_cleanly() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut failures = Vec::new();
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "sql") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            count += 1;
            let parse = diver_sql::parse(line);
            assert_eq!(parse.syntax().to_string(), line, "lossless");
            if let Some(e) = parse.errors().first() {
                failures.push(format!(
                    "{}: {line}\n    {} at {:?}",
                    path.file_name().unwrap().to_string_lossy(),
                    e.message,
                    e.range
                ));
            }
            let unknown = parse
                .syntax()
                .descendants()
                .any(|n| n.kind() == diver_sql::SyntaxKind::UNKNOWN_STMT);
            if unknown {
                failures.push(format!("unknown statement: {line}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {count} statements failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
