//! Fuzzers for the small languages people type: the search box, condition
//! cards, key strokes, view-action templates, pasted values, SQL expression
//! completion, and the conditional-format rules. None may panic on any text,
//! and ranges they hand back must stay inside the text on character
//! boundaries.

use proptest::prelude::*;

use crate::domain::documents::Engine;
use crate::domain::{actions, expression_completion, keystroke, pasted, query};

/// Characters the languages give meaning to, plus letters, digits, spaces
/// and a few multi-byte characters to land offsets inside.
const ALPHABET: &str = r#"[a-zA-Z0-9 _\-:=<>!|"'.,()\[\]{}$#%*+/\\@~^&?;\t\nöß日🙂İ]{0,40}"#;

fn char_offsets(text: &str) -> impl Iterator<Item = usize> + '_ {
    text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len()))
}

fn assert_range(text: &str, range: &std::ops::Range<usize>, what: &str) {
    assert!(
        range.start <= range.end && range.end <= text.len(),
        "{what}: range {range:?} is outside {text:?}"
    );
    assert!(
        text.is_char_boundary(range.start) && text.is_char_boundary(range.end),
        "{what}: range {range:?} splits a character of {text:?}"
    );
}

/// A search or condition text made of the search language's pieces.
fn search_text() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        proptest::sample::select(&[
            "north", "-", "|", " | ", ":", "=", "!=", ">", ">=", "<", "<=", "..", "\"", "'", "amount", "\"amount\"",
            "-name:", "2024", "2024-01", "2024-02-30", "-3", "1e400", "99999999999999999999999999999999999999999",
            "0.5", ".", "today", "null", "!", "*", "日本", " ", "  ", "\t",
        ][..])
        .prop_map(str::to_owned),
        "[a-z0-9]{1,4}",
    ];
    proptest::collection::vec(piece, 0..12).prop_map(|pieces| pieces.concat())
}

fn check_search(text: &str) {
    let _ = query::parse(text);
    let today = chrono::NaiveDate::from_ymd_opt(2024, 2, 29).unwrap();
    for column in ["amount", "name", ""] {
        let _ = query::parse_condition(text, column, today);
    }
    let _ = query::parse_number(text);
    let _ = query::date_period(text);
    for word in text.split_whitespace() {
        let _ = query::date_period(word);
        let _ = query::parse_number(word);
    }
    let highlights = query::highlight_terms(text);
    let _ = highlights;
    let terms: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    for range in query::highlight_ranges(text, &terms) {
        assert_range(text, &range, "highlight_ranges");
    }
    for offset in char_offsets(text) {
        if let Some(found) = query::column_name_query(text, offset) {
            assert_range(text, &found.range, "column_name_query");
        }
    }
    let names = ["amount", "Name", "日本", "a b", ""];
    let _ = query::rank_columns(&names, text);
    let _ = query::resolve_column(names, text);
}

fn check_keystroke(text: &str) {
    if let Ok(canonical) = crate::application::keymap::canonical_keystroke(text) {
        // A canonical stroke reads back as itself.
        assert_eq!(
            crate::application::keymap::canonical_keystroke(&canonical).as_deref(),
            Ok(canonical.as_str()),
            "{text:?} canonicalizes to {canonical:?}, which does not"
        );
        let _ = keystroke::display(&canonical);
        let _ = crate::ui::platform::KeyEvent::parse(&canonical);
    }
    let _ = keystroke::display(text);
    let _ = crate::ui::platform::KeyEvent::parse(text);
}

fn check_expression_completion(text: &str) {
    let catalog = expression_completion::ExpressionCatalog::new([
        ("amount".to_owned(), Some("BIGINT".to_owned())),
        ("Region Name".to_owned(), Some("VARCHAR".to_owned())),
        ("日本".to_owned(), None),
    ]);
    for engine in [Engine::DuckDb, Engine::Polars] {
        let dialect = expression_completion::dialect(engine);
        for offset in char_offsets(text) {
            for completion in expression_completion::completions(text, offset, &catalog, dialect) {
                assert_range(text, &completion.range, "expression completion");
            }
        }
    }
}

/// The rule texts of the presets, filled in for a small table.
fn rule_corpus() -> Vec<String> {
    let columns = format_columns();
    let mut rules: Vec<String> = diver_format::PRESETS.iter().map(|preset| preset.text(&columns, Some("sales"))).collect();
    rules.extend(
        [
            "if c > \"target\" then bg:red , border:1px, border_weight:2px, text:green",
            "bar(c, min: 0, max: 100, color: #63be7b)",
            "scale(c, #f8696b, #ffeb84, #63be7b)",
            "if c contains 'x' and not (\"sales\" between 1 and 5) then bold, tooltip: '{c:.2f} of {sales:,}'",
            "icons(c, arrows) if row() % 2 = 0 then bg: #eee",
            "spark(\"trend\") if \"when\" > today() - 7 then italic",
            "display: '{c:>10}' if c is null then text: gray",
        ]
        .map(str::to_owned),
    );
    rules
}

fn format_columns() -> Vec<diver_format::Column> {
    use diver_format::{Column, ValueKind};
    vec![
        Column::new("name", ValueKind::Text),
        Column::new("sales", ValueKind::Number),
        Column::new("target", ValueKind::Number),
        Column::new("trend", ValueKind::List),
        Column::new("when", ValueKind::Date),
        Column::new("ok", ValueKind::Bool),
        Column::new("info", ValueKind::Struct).with_fields(vec![Column::new("a", ValueKind::Number)]),
    ]
}

fn format_value() -> impl Strategy<Value = diver_format::Value> {
    use diver_format::Value;
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        prop_oneof![
            Just(0.0),
            Just(-0.0),
            Just(f64::NAN),
            Just(f64::INFINITY),
            Just(f64::NEG_INFINITY),
            Just(f64::MAX),
            Just(f64::MIN_POSITIVE),
            Just(1e300),
            -1e6..1e6f64,
        ]
        .prop_map(Value::Number),
        proptest::sample::select(&["", "x", "日本", "2024-02-29", "2024-13-45", "9999-12-31 23:59:59", "-1", "NaN"][..])
            .prop_map(|text| Value::Text(text.to_owned())),
    ];
    leaf.prop_recursive(2, 8, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::List),
            proptest::collection::vec(("[a-c]", inner), 0..3).prop_map(Value::Struct),
        ]
    })
}

/// Checks a rule text and runs it over `rows`, and every editor service at
/// every offset.
fn check_rules(text: &str, rows: &[Vec<diver_format::Value>]) {
    use diver_format::{TableStats, ValuesRow};
    let columns = format_columns();
    let checked = diver_format::check(text, &columns);
    for diagnostic in &checked.diagnostics {
        assert_range(text, &diagnostic.span, "diagnostic");
    }
    let mut by_column = vec![Vec::new(); columns.len()];
    for row in rows {
        for (column, value) in row.iter().enumerate() {
            by_column[column].push(value.clone());
        }
    }
    let stats = TableStats::new(&by_column);
    for (index, row) in rows.iter().enumerate() {
        for target in 0..columns.len() {
            let _ = checked.program.style(target, &ValuesRow { values: row, index }, &stats);
            let _ = checked.program.needs(target);
        }
    }
    for (range, _) in diver_format::help::highlight(text) {
        assert_range(text, &range, "highlight");
    }
    for (range, _) in diver_format::help::color_spans(text) {
        assert_range(text, &range, "color span");
    }
    for offset in char_offsets(text) {
        let _ = diver_format::complete(text, offset, &columns);
        if let Some(range) = diver_format::hex_at(text, offset) {
            assert_range(text, &range, "hex_at");
        }
        let _ = diver_format::help::hover(text, offset, &columns);
        // The range is the argument's, within the signature.
        if let Some((label, Some(range), _)) = diver_format::help::signature(text, offset) {
            assert_range(&label, &range, "signature");
        }
    }
}

/// A rule from the corpus, cut, spliced or shuffled at random offsets.
fn mutated_rule() -> impl Strategy<Value = String> {
    (any::<prop::sample::Index>(), any::<usize>(), any::<usize>(), 0..4u8, search_text()).prop_map(
        |(index, a, b, kind, insert)| {
            let corpus = rule_corpus();
            let rule = &corpus[index.index(corpus.len())];
            let floor = |i: usize| {
                let mut i = i % (rule.len() + 1);
                while !rule.is_char_boundary(i) {
                    i -= 1;
                }
                i
            };
            let (i, j) = (floor(a), floor(b));
            let (lo, hi) = (i.min(j), i.max(j));
            match kind {
                0 => rule[..lo].to_owned(),
                1 => format!("{}{}", &rule[..lo], &rule[hi..]),
                2 => format!("{}{}{}", &rule[hi..], &rule[lo..hi], &rule[..lo]),
                _ => format!("{}{insert}{}", &rule[..lo], &rule[lo..]),
            }
        },
    )
}

proptest! {
    #![proptest_config(super::config(2_000))]

    #[test]
    #[ignore = "stress test: run with scripts/stress.ps1"]
    fn stress_search_language(text in prop_oneof![search_text(), ALPHABET, any::<String>()]) {
        check_search(&text);
    }

    #[test]
    #[ignore = "stress test: run with scripts/stress.ps1"]
    fn stress_keystrokes(text in prop_oneof![
        r"((ctrl|alt|shift|win|cmd|super|platform|CTRL|Shift)[-+]){0,4}([a-z0-9]|f[0-9]{1,2}|enter|space|pgup|pgdn|left|tab|escape|-|\+|,|\.|/|`|=|)",
        any::<String>(),
    ]) {
        check_keystroke(&text);
    }

    #[test]
    #[ignore = "stress test: run with scripts/stress.ps1"]
    fn stress_action_templates(
        template in prop_oneof![
            r#"(https?://|cmd /c |explorer |"[a-z ]+" |)([a-z/ ]|\{[a-zA-Z ]{0,6}\}?|\{\{|\}\}|%|"|'|日){0,20}"#,
            any::<String>(),
        ],
        values in proptest::collection::vec(("[a-zA-Z ]{0,6}", any::<String>()), 0..4),
    ) {
        let _ = actions::launch_with_values(&template, &values);
    }

    #[test]
    #[ignore = "stress test: run with scripts/stress.ps1"]
    fn stress_pasted_values(text in prop_oneof![r#"(["\t\r\n ]|[a-c]|""|日){0,40}"#, any::<String>()], shown in 0..5usize) {
        let values = pasted::pasted_values(&text);
        let mut unique = values.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), values.len(), "pasted values repeat");
        assert!(values.iter().all(|value| !value.is_empty()), "a pasted value is blank");
        let _ = pasted::preview(&values, shown);
    }

    #[test]
    #[ignore = "stress test: run with scripts/stress.ps1"]
    fn stress_expression_completion(text in prop_oneof![
        r#"(sum|count|\(|\)|,|"|'|amount|"Region Name"|case|when|then|end| |\.|::|-|\*|[a-z]){0,14}"#,
        ALPHABET,
    ]) {
        check_expression_completion(&text);
    }

    #[test]
    #[ignore = "stress test: run with scripts/stress.ps1"]
    fn stress_format_rules(
        text in prop_oneof![3 => mutated_rule(), 1 => ALPHABET, 1 => any::<String>()],
        rows in proptest::collection::vec(proptest::collection::vec(format_value(), 7), 0..4),
    ) {
        check_rules(&text, &rows);
    }

    #[test]
    #[ignore = "stress test: run with scripts/stress.ps1"]
    fn stress_field_formats(
        format in prop_oneof![
            r"[<>^=]?[+\- ]?[#0]?[0-9]{0,3}[,_]?(\.[0-9]{0,3})?[bcdeEfFgGnosxX%]?",
            r"[0-9]{2,12}(\.[0-9]{1,12})?[ef%]",
            any::<String>(),
        ],
        value in format_value(),
    ) {
        if let Ok(field) = diver_format::template::FieldFormat::parse(&format) {
            let _ = field.apply(&value);
        }
        if let Ok(whole) = diver_format::format::Format::parse(&format) {
            let _ = whole.apply(&value);
        }
    }
}
