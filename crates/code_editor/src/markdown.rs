//! A small Markdown reader for hover text, signature documentation and help
//! pages: headings, paragraphs, lists, fenced code, quotes, rules and pipe
//! tables, with bold, italic, code and link spans inside them. It reads what
//! those texts use and is not a full CommonMark parser.

use std::ops::Range;

/// How a span of inline text is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Bold,
    Italic,
    Code,
    Link,
}

/// Text with styled byte ranges, which may nest (bold code).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub spans: Vec<(Range<usize>, Style)>,
    /// The target of each link span, in the same order as the link spans.
    pub links: Vec<(Range<usize>, String)>,
}

impl Inline {
    /// Text drawn as it is, with no styles.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading(u8, Inline),
    Paragraph(Inline),
    /// A list item: its nesting depth from 0, and its number when ordered.
    Item {
        depth: usize,
        number: Option<u32>,
        text: Inline,
    },
    Code {
        lang: String,
        text: String,
    },
    Quote(Inline),
    Rule,
    Table {
        header: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
}

/// Reads `md` into blocks.
pub fn parse(md: &str) -> Vec<Block> {
    let lines: Vec<&str> = md.lines().collect();
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut i = 0;
    let flush = |para: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !para.is_empty() {
            blocks.push(Block::Paragraph(inline(&join_lines(para))));
            para.clear();
        }
    };
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut para, &mut blocks);
            i += 1;
            continue;
        }
        if let Some(fence) = fence(trimmed) {
            flush(&mut para, &mut blocks);
            let lang = trimmed[fence.len()..].trim().to_string();
            let indent = line.len() - line.trim_start().len();
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with(fence) {
                let l = lines[i];
                let strip = l.len() - l.trim_start().len();
                code.push(&l[strip.min(indent)..]);
                i += 1;
            }
            i += 1;
            blocks.push(Block::Code {
                lang,
                text: code.join("\n"),
            });
            continue;
        }
        if is_rule(trimmed) {
            flush(&mut para, &mut blocks);
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }
        if let Some((level, rest)) = heading(trimmed) {
            flush(&mut para, &mut blocks);
            blocks.push(Block::Heading(level, inline(rest)));
            i += 1;
            continue;
        }
        if trimmed.starts_with('|') && lines.get(i + 1).is_some_and(|next| is_table_rule(next.trim())) {
            flush(&mut para, &mut blocks);
            let header = cells(trimmed);
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                rows.push(cells(lines[i].trim()));
                i += 1;
            }
            blocks.push(Block::Table { header, rows });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            flush(&mut para, &mut blocks);
            let mut quote = vec![rest.trim_start()];
            i += 1;
            while i < lines.len()
                && let Some(rest) = lines[i].trim().strip_prefix('>')
            {
                quote.push(rest.trim_start());
                i += 1;
            }
            blocks.push(Block::Quote(inline(&join_lines(&quote))));
            continue;
        }
        if let Some((number, rest)) = list_marker(trimmed) {
            flush(&mut para, &mut blocks);
            let depth = (line.len() - line.trim_start().len()) / 2;
            let mut item = vec![rest];
            i += 1;
            // Lines indented under the item, not starting a block, continue it.
            while i < lines.len() {
                let next = lines[i];
                let t = next.trim();
                let indented = next.starts_with(' ') || next.starts_with('\t');
                if t.is_empty() || !indented || list_marker(t).is_some() || fence(t).is_some() {
                    break;
                }
                item.push(t);
                i += 1;
            }
            blocks.push(Block::Item {
                depth,
                number,
                text: inline(&join_lines(&item)),
            });
            continue;
        }
        para.push(line.trim_start());
        i += 1;
    }
    flush(&mut para, &mut blocks);
    blocks
}

/// Joins a paragraph's lines with spaces; a line ending in two spaces or a
/// backslash breaks the line there.
fn join_lines(lines: &[&str]) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let hard = line.ends_with("  ") || line.ends_with('\\');
        let line = line.strip_suffix('\\').unwrap_or(line).trim_end();
        out.push_str(line);
        if i + 1 < lines.len() {
            out.push(if hard { '\n' } else { ' ' });
        }
    }
    out
}

fn fence(line: &str) -> Option<&'static str> {
    if line.starts_with("```") {
        Some("```")
    } else if line.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

fn is_rule(line: &str) -> bool {
    let chars: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    chars.len() >= 3 && ['-', '*', '_'].iter().any(|m| chars.iter().all(|c| c == m))
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    let rest = &line[level..];
    ((1..=6).contains(&level) && (rest.is_empty() || rest.starts_with(' ')))
        .then(|| (level as u8, rest.trim().trim_end_matches('#').trim_end()))
}

fn list_marker(line: &str) -> Option<(Option<u32>, &str)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some((None, rest));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && digits < 10 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((line[..digits].parse().ok(), rest));
        }
    }
    None
}

fn is_table_rule(line: &str) -> bool {
    line.starts_with('|')
        && line.contains('-')
        && line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn cells(line: &str) -> Vec<Inline> {
    let inner = line.trim().trim_start_matches('|');
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    // A `|` inside a code span or after a backslash is part of the cell.
    let mut out = Vec::new();
    let mut cell = String::new();
    let mut in_code = false;
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cell.push('|');
                chars.next();
            }
            '`' => {
                in_code = !in_code;
                cell.push(c);
            }
            '|' if !in_code => out.push(inline(std::mem::take(&mut cell).trim())),
            _ => cell.push(c),
        }
    }
    out.push(inline(cell.trim()));
    out
}

/// Reads the spans in one block's text.
pub fn inline(src: &str) -> Inline {
    let mut out = Inline::default();
    let mut open: Vec<(Style, usize, &'static str)> = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < src.len() {
        let rest = &src[i..];
        let c = rest.chars().next().unwrap();
        match c {
            '\\' if rest[1..].chars().next().is_some_and(|n| n.is_ascii_punctuation()) => {
                let n = rest[1..].chars().next().unwrap();
                out.text.push(n);
                i += 1 + n.len_utf8();
            }
            '`' => {
                let ticks = rest.chars().take_while(|c| *c == '`').count();
                let marker = &rest[..ticks];
                if let Some(end) = rest[ticks..].find(marker) {
                    let code = &rest[ticks..ticks + end];
                    // One space just inside the ticks pads code that starts or ends with a tick.
                    let code = if code.len() > 2 && code.starts_with(' ') && code.ends_with(' ') {
                        &code[1..code.len() - 1]
                    } else {
                        code
                    };
                    let start = out.text.len();
                    out.text.push_str(code);
                    out.spans.push((start..out.text.len(), Style::Code));
                    i += ticks + end + ticks;
                } else {
                    out.text.push_str(marker);
                    i += ticks;
                }
            }
            '[' => match link(rest) {
                Some((label, target, used)) => {
                    let inner = inline(label);
                    let start = out.text.len();
                    out.text.push_str(&inner.text);
                    for (r, style) in inner.spans {
                        out.spans.push((r.start + start..r.end + start, style));
                    }
                    let range = start..out.text.len();
                    out.spans.push((range.clone(), Style::Link));
                    out.links.push((range, target.to_string()));
                    i += used;
                }
                None => {
                    out.text.push('[');
                    i += 1;
                }
            },
            '*' | '_' => {
                let double = rest.starts_with("**") || rest.starts_with("__");
                let marker: &'static str = match (c, double) {
                    ('*', true) => "**",
                    ('*', false) => "*",
                    ('_', true) => "__",
                    _ => "_",
                };
                let style = if double { Style::Bold } else { Style::Italic };
                let before = src[..i].chars().next_back();
                let after = rest[marker.len()..].chars().next();
                // Underscores inside a word, like snake_case, are just text.
                let word = |ch: Option<char>| ch.is_some_and(|ch| ch.is_alphanumeric());
                let intraword = c == '_' && word(before) && word(after);
                let can_close = !before.is_some_and(char::is_whitespace) && !intraword;
                let can_open = after.is_some_and(|a| !a.is_whitespace()) && !intraword;
                if let Some(pos) = open.iter().rposition(|(_, _, m)| *m == marker)
                    && can_close
                {
                    let (style, start, _) = open.remove(pos);
                    open.truncate(pos);
                    if start < out.text.len() {
                        out.spans.push((start..out.text.len(), style));
                    }
                    i += marker.len();
                } else if can_open && has_closer(&bytes[i + marker.len()..], marker) {
                    open.push((style, out.text.len(), marker));
                    i += marker.len();
                } else {
                    out.text.push_str(marker);
                    i += marker.len();
                }
            }
            _ => {
                out.text.push(c);
                i += c.len_utf8();
            }
        }
    }
    out.spans.sort_by_key(|(r, _)| (r.start, std::cmp::Reverse(r.end)));
    out
}

fn has_closer(rest: &[u8], marker: &str) -> bool {
    rest.windows(marker.len()).any(|w| w == marker.as_bytes())
}

/// `[label](target)` at the start of `s`: the label, the target and the
/// bytes used.
fn link(s: &str) -> Option<(&str, &str, usize)> {
    let mut depth = 0;
    let mut close = None;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let rest = s[close + 1..].strip_prefix('(')?;
    let end = rest.find(')')?;
    Some((&s[1..close], rest[..end].trim(), close + 2 + end + 1))
}

/// Escapes `text` so it reads back as itself.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if !out.is_empty() {
            // A hard break keeps the line as it was.
            out.push_str("\\\n");
        }
        let t = line.trim_start();
        let lead = line.len() - t.len();
        out.push_str(&line[..lead]);
        if heading(t).is_some()
            || list_marker(t).is_some()
            || t.starts_with('>')
            || t.starts_with('|')
            || fence(t).is_some()
            || is_rule(t)
        {
            out.push('\\');
        }
        for c in t.chars() {
            if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']') {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(i: &Inline) -> Vec<(&str, Style)> {
        i.spans.iter().map(|(r, s)| (&i.text[r.clone()], *s)).collect()
    }

    #[test]
    fn blocks() {
        let md = "# Title\n\nFirst line\njoined.\n\n- one\n- two\n  more\n  1. nested\n\n```sql\nSELECT 1\n```\n---\n> quoted\n\n| Key | Does |\n|---|---|\n| `F1` | Help \\| more |";
        let blocks = parse(md);
        assert_eq!(blocks[0], Block::Heading(1, Inline::plain("Title")));
        assert_eq!(blocks[1], Block::Paragraph(Inline::plain("First line joined.")));
        assert!(matches!(&blocks[2], Block::Item { depth: 0, number: None, text } if text.text == "one"));
        assert!(matches!(&blocks[3], Block::Item { text, .. } if text.text == "two more"));
        assert!(matches!(&blocks[4], Block::Item { depth: 1, number: Some(1), text } if text.text == "nested"));
        assert_eq!(blocks[5], Block::Code { lang: "sql".into(), text: "SELECT 1".into() });
        assert_eq!(blocks[6], Block::Rule);
        assert_eq!(blocks[7], Block::Quote(Inline::plain("quoted")));
        let Block::Table { header, rows } = &blocks[8] else { panic!("{:?}", blocks[8]) };
        assert_eq!(header.len(), 2);
        assert_eq!(rows[0][0].text, "F1");
        assert_eq!(rows[0][1].text, "Help | more");
    }

    #[test]
    fn spans() {
        let i = inline("**SELECT**: rows from `tbl`, *really* [docs](https://x.y) a_b_c");
        assert_eq!(i.text, "SELECT: rows from tbl, really docs a_b_c");
        assert_eq!(
            styles(&i),
            [("SELECT", Style::Bold), ("tbl", Style::Code), ("really", Style::Italic), ("docs", Style::Link)]
        );
        assert_eq!(i.links[0].1, "https://x.y");
        // Unclosed markers and lone stars stay text.
        assert_eq!(inline("2 * 3 and **open").text, "2 * 3 and **open");
        assert_eq!(inline("\\*not\\*").text, "*not*");
        assert_eq!(inline("``a ` b``").text, "a ` b");
    }

    #[test]
    fn escape_reads_back_as_itself() {
        for text in ["a * b_c [x](y) `q`", "- not a list\n# not a heading", "plain"] {
            let blocks = parse(&escape(text));
            let Block::Paragraph(i) = &blocks[0] else { panic!("{blocks:?}") };
            assert_eq!(i.text, text);
            assert!(i.spans.is_empty());
        }
    }
}
