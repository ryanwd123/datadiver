//! Properties: on any input the parser never panics, the tree reproduces the
//! text, the fuel limit is never hit, and completion works at every offset.

mod common;

use diver_sql::{TextSize, complete, complete_with, parse};
use proptest::prelude::*;

fn check_text(text: &str) {
    let p = parse(text);
    assert_eq!(p.syntax().to_string(), text, "round trip");
    assert!(
        !p.errors().iter().any(|e| e.message.contains("stuck")),
        "parser got stuck on {text:?}"
    );
    let tokens = diver_sql::tokenize(text);
    let total: u32 = tokens.iter().map(|t| u32::from(t.len)).sum();
    assert_eq!(total as usize, text.len());
}

fn check_completion_everywhere(text: &str) {
    let catalog = common::catalog();
    for (offset, _) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        let offset = TextSize::new(offset as u32);
        let ctx = complete(text, offset);
        assert!(ctx.replace_range.end() <= TextSize::of(text));
        let _ = complete_with(text, offset, &catalog);
    }
}

/// The curated corpus, one statement per line.
fn corpus() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        out.extend(
            text.lines()
                .filter(|l| !l.trim().is_empty())
                .map(str::to_string),
        );
    }
    out.sort();
    out
}

fn floor_char(text: &str, i: usize) -> usize {
    let mut i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn any_string(text in any::<String>()) {
        check_text(&text);
    }

    #[test]
    fn sql_alphabet(text in r#"[a-z_ 0-9'"(),.;*=<>+\-/:\[\]{}$?|!~@#%^&\n]{0,60}"#) {
        check_text(&text);
        check_completion_everywhere(&text);
    }

    #[test]
    fn keyword_soup(words in proptest::collection::vec(
        prop_oneof![
            Just("select"), Just("from"), Just("where"), Just("("), Just(")"), Just(","), Just("a"),
            Just("join"), Just("on"), Just("group"), Just("by"), Just("with"), Just("as"), Just("case"),
            Just("when"), Just("then"), Just("end"), Just("create"), Just("table"), Just("insert"),
            Just("into"), Just("values"), Just("1"), Just("'x'"), Just("*"), Just("."), Just("::"),
            Just("over"), Just("partition"), Just("order"), Just("pivot"), Just("using"), Just(";"),
            Just("lambda"), Just(":"), Just("["), Just("]"), Just("{"), Just("}"), Just("not"), Just("in"),
        ],
        0..30,
    )) {
        let text = words.join(" ");
        check_text(&text);
        check_completion_everywhere(&text);
    }

    #[test]
    fn corpus_mutations(index in any::<prop::sample::Index>(), a in any::<usize>(), b in any::<usize>(), kind in 0..3u8) {
        let corpus = corpus();
        let stmt = &corpus[index.index(corpus.len())];
        let i = floor_char(stmt, a % (stmt.len() + 1));
        let j = floor_char(stmt, b % (stmt.len() + 1));
        let (lo, hi) = (i.min(j), i.max(j));
        let text = match kind {
            // truncation
            0 => stmt[..lo].to_string(),
            // deletion
            1 => format!("{}{}", &stmt[..lo], &stmt[hi..]),
            // swap two pieces
            _ => format!("{}{}{}", &stmt[hi..], &stmt[lo..hi], &stmt[..lo]),
        };
        check_text(&text);
        check_completion_everywhere(&text);
    }
}

#[test]
fn completion_at_every_offset_of_the_corpus() {
    for stmt in corpus() {
        check_completion_everywhere(&stmt);
    }
}

#[test]
fn deep_nesting_does_not_overflow() {
    let text = format!("SELECT {}1{}", "(".repeat(200), ")".repeat(200));
    check_text(&text);
    let text = format!("SELECT {}", "[".repeat(300));
    check_text(&text);
}

#[test]
fn very_deep_nesting_is_cut_off() {
    for open in ["(", "[", "{", "CASE WHEN ", "NOT ", "- ", "(SELECT "] {
        let text = format!("SELECT {}", open.repeat(5000));
        check_text(&text);
    }
}
