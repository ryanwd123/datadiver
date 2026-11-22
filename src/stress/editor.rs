//! Fuzzer for the code editor's editing commands: random multi-cursor
//! selections and commands over text with tabs, CRLF, combining marks and
//! wide characters. Every edit and selection must stay inside the text on
//! character boundaries, and undoing every step must give back the text it
//! started from, and redoing them the text it ended with.

use code_editor::commands::{self, IndentOptions, TypeContext};
use code_editor::movement;
use code_editor::selection::{Selection, Selections};
use code_editor::text::{Buffer, Transaction};
use proptest::prelude::*;

#[derive(Clone, Debug)]
enum Op {
    Select(Vec<(usize, usize)>),
    Insert(String),
    Paste(String, bool),
    Type(char),
    Backspace,
    BackspacePairs,
    Delete,
    DeleteWordLeft,
    DeleteWordRight,
    Newline,
    Tab,
    Indent,
    Outdent,
    Cut,
    LineComment,
    BlockComment,
    MoveLines(bool),
    CopyLines(bool),
    DeleteLines,
    SelectLine,
    LineAbove,
    LineBelow,
    SelectAll,
    /// Moves the primary cursor: word left/right, home, end, up, down,
    /// grapheme left/right.
    Move(u8, bool),
}

fn piece() -> impl Strategy<Value = String> {
    proptest::sample::select(&[
        "select a, b\n", "\tindented\n", "    four\n", "-- comment\n", "/* x */", "'str'", "\"q\"", "日本", "🙂",
        "e\u{301}", "\r\n", "\n", "(", ")", "[]", "{}", " ", "x", "\t", "",
    ][..])
    .prop_map(str::to_owned)
}

fn text() -> impl Strategy<Value = String> {
    proptest::collection::vec(piece(), 0..10).prop_map(|pieces| pieces.concat())
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => proptest::collection::vec((any::<usize>(), any::<usize>()), 1..4).prop_map(Op::Select),
        2 => text().prop_map(Op::Insert),
        1 => (text(), any::<bool>()).prop_map(|(text, lines)| Op::Paste(text, lines)),
        3 => proptest::sample::select(&['a', '(', ')', '\'', '"', '[', ']', '{', '}', ' ', '\t', '日', '-', '/', '*'][..]).prop_map(Op::Type),
        2 => Just(Op::Backspace),
        1 => Just(Op::BackspacePairs),
        1 => Just(Op::Delete),
        1 => Just(Op::DeleteWordLeft),
        1 => Just(Op::DeleteWordRight),
        1 => Just(Op::Newline),
        1 => Just(Op::Tab),
        1 => Just(Op::Indent),
        1 => Just(Op::Outdent),
        1 => Just(Op::Cut),
        1 => Just(Op::LineComment),
        1 => Just(Op::BlockComment),
        1 => any::<bool>().prop_map(Op::MoveLines),
        1 => any::<bool>().prop_map(Op::CopyLines),
        1 => Just(Op::DeleteLines),
        1 => Just(Op::SelectLine),
        1 => Just(Op::LineAbove),
        1 => Just(Op::LineBelow),
        1 => Just(Op::SelectAll),
        3 => (0..8u8, any::<bool>()).prop_map(|(kind, extend)| Op::Move(kind, extend)),
    ]
}

fn assert_offset(buf: &Buffer, offset: usize, what: &str) {
    let text = buf.text();
    assert!(offset <= text.len(), "{what} {offset} is past the end of {text:?}");
    assert!(text.is_char_boundary(offset), "{what} {offset} splits a character of {text:?}");
}

fn assert_selections(buf: &Buffer, sels: &Selections, what: &str) {
    for s in sels.iter() {
        assert_offset(buf, s.anchor, &format!("{what}: anchor"));
        assert_offset(buf, s.head, &format!("{what}: head"));
    }
    let starts: Vec<usize> = sels.iter().map(Selection::start).collect();
    assert!(starts.windows(2).all(|w| w[0] <= w[1]), "{what}: selections out of order: {sels:?}");
}

fn assert_transaction(buf: &Buffer, tx: &Transaction, what: &str) {
    let mut end = 0;
    for edit in &tx.edits {
        assert!(edit.start <= edit.end && edit.start >= end, "{what}: edits overlap or run backwards: {tx:?}");
        assert_offset(buf, edit.start, &format!("{what}: edit start"));
        assert_offset(buf, edit.end, &format!("{what}: edit end"));
        end = edit.end;
    }
}

fn check(initial: &str, ops: &[Op]) {
    let opts = IndentOptions::default();
    let mut buf = Buffer::new(initial);
    let start = buf.text();
    let mut sels = Selections::single(Selection::cursor(0));
    let mut undo: Vec<Transaction> = Vec::new();
    let mut auto_closed: Vec<(usize, usize)> = Vec::new();
    let pairs = [("(", ")"), ("[", "]"), ("{", "}"), ("'", "'"), ("\"", "\"")];
    for op in ops {
        let what = format!("{op:?} on {:?} with {sels:?}", buf.text());
        let mut closed = Vec::new();
        let outcome = match op {
            Op::Select(ranges) => {
                let len = buf.len();
                let ranges = ranges
                    .iter()
                    .map(|(a, b)| Selection::new(buf.floor_char(a % (len + 1)), buf.floor_char(b % (len + 1))))
                    .collect();
                sels = Selections::new(ranges, 0);
                None
            }
            Op::Insert(text) => Some(commands::insert(&sels, &code_editor::text::normalize_eol(text))),
            Op::Paste(text, lines) => Some(commands::paste(&buf, &sels, text, *lines)),
            Op::Type(c) => {
                let in_string = |_| false;
                let cx = TypeContext { pairs: &pairs, in_string_or_comment: &in_string, auto_closed: &auto_closed };
                let (outcome, new_closed) = commands::type_char(&buf, &sels, *c, &cx);
                closed = new_closed;
                Some(outcome)
            }
            Op::Backspace => Some(commands::backspace(&buf, &sels, opts)),
            Op::BackspacePairs => Some(commands::backspace_pairs(&buf, &sels, opts, &auto_closed)),
            Op::Delete => Some(commands::delete(&buf, &sels)),
            Op::DeleteWordLeft => Some(commands::delete_word_left(&buf, &sels)),
            Op::DeleteWordRight => Some(commands::delete_word_right(&buf, &sels)),
            Op::Newline => Some(commands::newline(&buf, &sels, commands::current_indent)),
            Op::Tab => Some(commands::tab(&buf, &sels, opts)),
            Op::Indent => Some(commands::indent_lines(&buf, &sels, opts)),
            Op::Outdent => Some(commands::outdent_lines(&buf, &sels, opts)),
            Op::Cut => Some(commands::cut(&buf, &sels).2),
            Op::LineComment => Some(commands::toggle_line_comment(&buf, &sels, "--")),
            Op::BlockComment => Some(commands::toggle_block_comment(&buf, &sels, "/*", "*/")),
            Op::MoveLines(down) => commands::move_lines(&buf, &sels, *down),
            Op::CopyLines(down) => Some(commands::copy_lines(&buf, &sels, *down)),
            Op::DeleteLines => Some(commands::delete_lines(&buf, &sels)),
            Op::SelectLine => {
                sels = commands::select_line(&buf, &sels);
                None
            }
            Op::LineAbove => Some(commands::insert_line_above(&buf, &sels)),
            Op::LineBelow => Some(commands::insert_line_below(&buf, &sels, |off| commands::current_indent(&buf, off))),
            Op::SelectAll => {
                sels = commands::select_all(&buf);
                None
            }
            Op::Move(kind, extend) => {
                let primary = sels.primary();
                let head = match kind {
                    0 => movement::word_left(&buf, primary.head),
                    1 => movement::word_right(&buf, primary.head),
                    2 => movement::home(&buf, primary.head),
                    3 => movement::line_end(&buf, primary.head),
                    4 => movement::vertical(&buf, primary.head, primary.goal, -1, opts.tab_size).0,
                    5 => movement::vertical(&buf, primary.head, primary.goal, 1, opts.tab_size).0,
                    6 => movement::prev_grapheme(&buf, primary.head),
                    _ => movement::next_grapheme(&buf, primary.head),
                };
                let anchor = if *extend { primary.anchor } else { head };
                sels = Selections::single(Selection::new(anchor, head));
                None
            }
        };
        if let Some(outcome) = outcome {
            assert_transaction(&buf, &outcome.tx, &what);
            let (inverse, _) = buf.apply(&outcome.tx);
            undo.push(inverse);
            sels = outcome.sels;
        }
        assert_selections(&buf, &sels, &what);
        auto_closed = closed;
        // The auto-closed pairs the editor would type over stay in the text.
        for &(open, close) in &auto_closed {
            assert_offset(&buf, open, &format!("{what}: auto-closed opener"));
            assert_offset(&buf, close, &format!("{what}: auto-closed closer"));
        }
    }
    let end = buf.text();
    let mut redo = Vec::new();
    for inverse in undo.iter().rev() {
        assert_transaction(&buf, inverse, "undo");
        redo.push(buf.apply(inverse).0);
    }
    assert_eq!(buf.text(), start, "undoing every step does not give back the text");
    for forward in redo.iter().rev() {
        assert_transaction(&buf, forward, "redo");
        buf.apply(forward);
    }
    assert_eq!(buf.text(), end, "redoing every step does not give back the edited text");
}

proptest! {
    #![proptest_config(super::config(3_000))]

    #[test]
    #[ignore = "stress test: run with scripts/stress.ps1"]
    fn stress_editor_commands(initial in text(), ops in proptest::collection::vec(op(), 1..25)) {
        check(&initial, &ops);
    }
}
