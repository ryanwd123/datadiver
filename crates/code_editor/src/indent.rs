//! Indentation: detecting a file's indentation, and the indentation Enter
//! inserts, driven by each language's `IndentRules`.

use crate::commands::{IndentOptions, current_indent};
use crate::lang::Lang;
use crate::movement::visual_col;
use crate::text::Buffer;

/// Guesses tabs or spaces and the tab size from leading whitespace, like
/// Monaco's `guessIndentation`. `None` when there is not enough indentation.
pub fn detect(buf: &Buffer) -> Option<IndentOptions> {
    let (mut tabs, mut spaces) = (0usize, 0usize);
    // Counts of indent width changes between consecutive indented lines.
    let mut deltas = [0usize; 9];
    let mut prev = 0usize;
    for l in 0..buf.line_count().min(10_000) {
        let line = buf.line(l);
        if line.trim().is_empty() {
            continue;
        }
        let lead = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
        if lead.starts_with('\t') {
            tabs += 1;
        } else if lead.starts_with(' ') {
            spaces += 1;
        }
        let width = lead.chars().take_while(|&c| c == ' ').count();
        if !lead.contains('\t') {
            let d = width.abs_diff(prev);
            if (2..=8).contains(&d) {
                deltas[d] += 1;
            }
            prev = width;
        }
    }
    if tabs == 0 && spaces == 0 {
        return None;
    }
    if tabs > spaces {
        return Some(IndentOptions {
            tab_size: 4,
            insert_spaces: false,
        });
    }
    // Prefer 4, then 2, then 8 on ties; multiples of a smaller size count for it.
    let score = |n: usize| deltas[n] + if n * 2 <= 8 { deltas[n * 2] } else { 0 };
    let size = [4, 2, 8, 3, 6, 5, 7]
        .into_iter()
        .max_by_key(|&n| (score(n), std::cmp::Reverse([4, 2, 8, 3, 6, 5, 7].iter().position(|&x| x == n))))
        .filter(|&n| score(n) > 0)
        .unwrap_or(4);
    // A size whose only evidence is its double (8 for 4) defers to the double.
    let size = if deltas[size] == 0 && size * 2 <= 8 && deltas[size * 2] > 0 { size * 2 } else { size };
    Some(IndentOptions {
        tab_size: size,
        insert_spaces: true,
    })
}

/// Fold regions by indentation (header line, last folded line) for headers in
/// `first..=last`: a line followed by more deeply indented lines folds them,
/// not counting trailing blank lines.
pub fn fold_ranges(buf: &Buffer, first: usize, last: usize, tab: usize) -> Vec<(usize, usize)> {
    let n = buf.line_count();
    let level = |l: usize| {
        let t = buf.line(l);
        (!t.trim().is_empty()).then(|| {
            let lead = t.len() - t.trim_start_matches([' ', '\t']).len();
            visual_col(&t, lead, tab)
        })
    };
    let mut out = Vec::new();
    for h in first..=last.min(n.saturating_sub(1)) {
        let Some(hl) = level(h) else { continue };
        let mut end = h;
        let mut l = h + 1;
        while l < n {
            match level(l) {
                Some(x) if x <= hl => break,
                Some(_) => end = l,
                None => {}
            }
            l += 1;
        }
        if end > h {
            out.push((h, end));
        }
    }
    out
}

/// What Enter at `off` inserts: the text, and where the caret goes in it.
pub struct Enter {
    pub text: String,
    pub caret: usize,
}

/// The Enter result at `off`. `strip_comment` gives the start of a trailing
/// comment in the line before `off`, if any.
pub fn enter(
    buf: &Buffer,
    off: usize,
    lang: Option<&Lang>,
    opts: IndentOptions,
    strip_comment: impl Fn(usize) -> Option<usize>,
) -> Enter {
    let base = current_indent(buf, off);
    let p = buf.pos(off);
    let line = buf.line(p.line);
    let line_start = buf.line_start(p.line);
    let code_end = strip_comment(off).map_or(p.col, |c| c.saturating_sub(line_start).min(p.col));
    let before = line[..code_end].trim_end();
    let after = line[p.col..].trim_start();
    let unit = opts.unit();
    let plain = |indent: String| Enter {
        caret: 1 + indent.len(),
        text: format!("\n{indent}"),
    };
    let Some(lang) = lang else { return plain(base) };

    let last = before.chars().last();
    let opens = |c: Option<char>| c.and_then(|c| lang.brackets.iter().find(|b| b.0 == c));
    // Between a bracket pair: the closer goes on its own line.
    if let Some(&(_, close)) = opens(last)
        && after.starts_with(close)
    {
        let inner = format!("{base}{unit}");
        return Enter {
            caret: 1 + inner.len(),
            text: format!("\n{inner}\n{base}"),
        };
    }
    if opens(last).is_some() || lang.indent.indent_after.iter().any(|s| before.ends_with(s)) {
        return plain(format!("{base}{unit}"));
    }
    let first = before.trim_start().split(|c: char| !c.is_alphanumeric() && c != '_').next();
    if after.is_empty() && first.is_some_and(|w| lang.indent.dedent_after.contains(&w)) {
        return plain(outdented(&base, opts));
    }
    plain(base)
}

/// `indent` one level shallower.
fn outdented(indent: &str, opts: IndentOptions) -> String {
    let width = visual_col(indent, indent.len(), opts.tab_size);
    let target = width.saturating_sub(opts.tab_size) / opts.tab_size * opts.tab_size;
    if opts.insert_spaces {
        " ".repeat(target)
    } else {
        "\t".repeat(target / opts.tab_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> IndentOptions {
        IndentOptions::default()
    }

    #[test]
    fn detects_spaces_and_tabs() {
        let two = Buffer::new("a:\n  b\n  c:\n    d\ne\n");
        assert_eq!(detect(&two).map(|o| (o.tab_size, o.insert_spaces)), Some((2, true)));
        let four = Buffer::new("def f():\n    if x:\n        y\n    z\n");
        assert_eq!(detect(&four).map(|o| (o.tab_size, o.insert_spaces)), Some((4, true)));
        let tabs = Buffer::new("a\n\tb\n\t\tc\n\td\n");
        assert_eq!(detect(&tabs).map(|o| o.insert_spaces), Some(false));
        assert!(detect(&Buffer::new("flat\ntext\n")).is_none());
    }

    #[test]
    fn folds_by_indentation() {
        let b = Buffer::new("a:\n  b\n\n  c\n\nd\n  e\n");
        assert_eq!(fold_ranges(&b, 0, 10, 4), vec![(0, 3), (5, 6)]);
        assert_eq!(fold_ranges(&b, 4, 10, 4), vec![(5, 6)]);
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_enter_rules() {
        let py = crate::lang::by_id("python");
        let enter_at = |text: &str| {
            let b = Buffer::new(text);
            let e = enter(&b, b.len(), py, opts(), |_| None);
            e.text
        };
        assert_eq!(enter_at("def f():"), "\n    ");
        assert_eq!(enter_at("    if x:  "), "\n        ");
        assert_eq!(enter_at("x = foo("), "\n    ");
        assert_eq!(enter_at("        return x"), "\n    ");
        assert_eq!(enter_at("    pass"), "\n");
        assert_eq!(enter_at("    passed = 1"), "\n    ");
        assert_eq!(enter_at("    x = 1"), "\n    ");
        // A trailing comment does not hide the colon.
        let b = Buffer::new("if x:  # why");
        let e = enter(&b, b.len(), py, opts(), |_| Some(7));
        assert_eq!(e.text, "\n    ");
        // Between brackets the closer moves to its own line.
        let b = Buffer::new("  f()");
        let e = enter(&b, 4, py, opts(), |_| None);
        assert_eq!((e.text.as_str(), e.caret), ("\n      \n  ", 7));
    }

    #[cfg(feature = "lang-sql")]
    #[test]
    fn sql_enter_rules() {
        let sql = crate::lang::by_id("sql");
        let b = Buffer::new("  WHERE id IN (");
        assert_eq!(enter(&b, b.len(), sql, opts(), |_| None).text, "\n      ");
        let b = Buffer::new("  SELECT a,");
        assert_eq!(enter(&b, b.len(), sql, opts(), |_| None).text, "\n  ");
    }
}
