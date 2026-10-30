//! DuckDB's own test SQL (`test/sql/**/*.test` of v1.5.6), fetched by
//! `tools/fetch_duckdb_corpus.ps1`. Reports how many statements parse
//! without errors and the largest failure groups, and fails below the
//! ratchet.
//!
//!     cargo test -p diver_sql --test duckdb_corpus -- --ignored --nocapture
//!
//! With `--features duckdb-diff`, only statements DuckDB's own parser accepts
//! count, so a failure means "DuckDB accepts it and we report an error", and
//! statement splitting is compared with `duckdb_extract_statements`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// The pass rate (in percent) below which the test fails. Ratchet it up as the grammar improves.
const RATCHET: f64 = 99.9;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/duckdb/test/sql")
}

fn test_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            test_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "test" || e == "test_slow") {
            out.push(path);
        }
    }
}

/// The SQL of the `statement ok`, `statement maybe` and `query` records of a
/// sqllogictest file.
fn records(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let head = lines[i].trim();
        let is_record = head.starts_with("statement ok") || head.starts_with("statement maybe") || head.starts_with("query ");
        i += 1;
        if !is_record {
            continue;
        }
        let mut sql = Vec::new();
        while i < lines.len() && !lines[i].trim().is_empty() && lines[i].trim() != "----" {
            sql.push(lines[i]);
            i += 1;
        }
        let text = sql.join("\n").trim().to_string();
        // sqllogictest substitutions and test-only paths
        let templated = text.contains("${") || text.contains("__TEST_DIR__") || text.contains("__WORKING_DIRECTORY__");
        let braces = text.contains('{') && regex_like_template(&text);
        if !text.is_empty() && !templated && !braces {
            out.push(text);
        }
    }
    out
}

/// `{name}` placeholders of foreach loops.
fn regex_like_template(text: &str) -> bool {
    let mut i = 0;
    while let Some(start) = text[i..].find('{').map(|p| p + i) {
        let rest = &text[start + 1..];
        let end = rest.find('}');
        if let Some(end) = end {
            let inner = &rest[..end];
            if !inner.is_empty() && inner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') && !inner.bytes().all(|b| b.is_ascii_digit()) {
                // `{a: 1}` is a struct; `{x}` alone is a template
                if !rest[end + 1..].trim_start().starts_with(':') {
                    return true;
                }
            }
        }
        i = start + 1;
    }
    false
}

/// What to group a failure by: the statement's first word and where it failed.
fn failure_key(sql: &str, error: &diver_sql::SyntaxError) -> String {
    let first = sql.split(|c: char| !c.is_alphanumeric() && c != '_').find(|w| !w.is_empty()).unwrap_or("?").to_uppercase();
    let start = usize::from(error.range.start()).min(sql.len());
    let end = usize::from(error.range.end()).min(sql.len());
    let at = sql.get(start..end).unwrap_or("");
    let at = if at.is_empty() {
        "<end>".to_string()
    } else if at.starts_with('\'') {
        "STRING".to_string()
    } else if at.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        "NUMBER".to_string()
    } else {
        at.to_uppercase()
    };
    format!("{first:10} at {at:12} {}", error.message)
}

#[cfg(feature = "duckdb-diff")]
mod duckdb_parser {
    use std::ffi::{CStr, CString};

    use duckdb::ffi;

    pub struct DuckParser {
        db: ffi::duckdb_database,
        con: ffi::duckdb_connection,
    }

    impl DuckParser {
        pub fn new() -> DuckParser {
            unsafe {
                let mut db = std::ptr::null_mut();
                let mut con = std::ptr::null_mut();
                assert_eq!(ffi::duckdb_open(std::ptr::null(), &mut db), ffi::DuckDBSuccess);
                assert_eq!(ffi::duckdb_connect(db, &mut con), ffi::DuckDBSuccess);
                DuckParser { db, con }
            }
        }

        /// The statements DuckDB splits `sql` into, or None when it can't parse it.
        pub fn statements(&self, sql: &str) -> Option<usize> {
            let c = CString::new(sql).ok()?;
            unsafe {
                let mut out = std::ptr::null_mut();
                let n = ffi::duckdb_extract_statements(self.con, c.as_ptr(), &mut out);
                let error = ffi::duckdb_extract_statements_error(out);
                let ok = error.is_null() || CStr::from_ptr(error).to_bytes().is_empty();
                ffi::duckdb_destroy_extracted(&mut out);
                (ok && n > 0).then_some(n as usize)
            }
        }
    }

    impl Drop for DuckParser {
        fn drop(&mut self) {
            unsafe {
                ffi::duckdb_disconnect(&mut self.con);
                ffi::duckdb_close(&mut self.db);
            }
        }
    }
}

#[test]
#[ignore = "needs tools/fetch_duckdb_corpus.ps1"]
fn duckdb_test_corpus() {
    let dir = corpus_dir();
    assert!(dir.exists(), "run tools/fetch_duckdb_corpus.ps1 first ({})", dir.display());
    let mut files = Vec::new();
    test_files(&dir, &mut files);
    files.sort();

    #[cfg(feature = "duckdb-diff")]
    let duck = duckdb_parser::DuckParser::new();
    let mut seen = HashSet::new();
    let mut total = 0usize;
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    #[cfg(feature = "duckdb-diff")]
    let mut split_mismatches = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        for sql in records(&text) {
            let key = sql.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches(';').to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            #[cfg(feature = "duckdb-diff")]
            {
                match duck.statements(&sql) {
                    None => continue,
                    Some(n) => {
                        // DuckDB expands some statements into several (PIVOT,
                        // ALTER ... DEFAULT nextval()), so only splitting more
                        // than DuckDB is a mismatch.
                        let ours = diver_sql::statements(&sql).len();
                        if ours > n {
                            split_mismatches.push(format!("duckdb {n}, ours {ours}: {sql}"));
                        }
                        if n != 1 {
                            continue;
                        }
                    }
                }
            }
            if diver_sql::statements(&sql).len() != 1 {
                continue;
            }
            total += 1;
            let parse = diver_sql::parse(&sql);
            assert_eq!(parse.syntax().to_string(), sql);
            if let Some(e) = parse.errors().first() {
                groups.entry(failure_key(&sql, e)).or_default().push(sql);
            } else if parse.syntax().descendants().any(|n| n.kind() == diver_sql::SyntaxKind::UNKNOWN_STMT) {
                let first = sql.split_whitespace().next().unwrap_or("").to_uppercase();
                groups.entry(format!("{first:10} unknown statement")).or_default().push(sql);
            }
        }
    }
    let failed: usize = groups.values().map(Vec::len).sum();
    let rate = 100.0 * (total - failed) as f64 / total.max(1) as f64;
    println!("{total} distinct statements; {} parse cleanly ({rate:.2}%)\n", total - failed);
    let mut sorted: Vec<_> = groups.iter().collect();
    sorted.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (key, items) in sorted.iter().take(60) {
        println!("{:6}  {key}", items.len());
        for sql in items.iter().take(3) {
            let one_line: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
            println!("          {}", one_line.chars().take(160).collect::<String>());
        }
    }
    if let Ok(path) = std::env::var("DIVER_SQL_FAILURES") {
        let mut out = String::new();
        for (key, items) in &sorted {
            for sql in items.iter() {
                out.push_str(&format!("-- {key}\n{sql};\n\n"));
            }
        }
        std::fs::write(path, out).unwrap();
    }
    #[cfg(feature = "duckdb-diff")]
    {
        println!("\n{} statement-splitting mismatches", split_mismatches.len());
        for m in split_mismatches.iter().take(10) {
            println!("    {}", m.chars().take(160).collect::<String>());
        }
    }
    assert!(rate >= RATCHET, "pass rate {rate:.2}% is below the ratchet {RATCHET}%");
}
