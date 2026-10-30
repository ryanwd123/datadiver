//! Tree-sitter parsing and highlighting. Grammars and their highlight queries
//! are compiled the first time a document uses them; every edit updates the
//! tree incrementally, and highlighting runs only over the lines being shown.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ropey::Rope;
use tree_sitter::{InputEdit, Language, Node, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};

use crate::lang::Lang;
use crate::render::MAX_RENDER_BYTES;
use crate::text::{Change, Pos};
use crate::theme::Ink;

/// Files bigger than this get their first parse on a worker thread.
pub const BACKGROUND_PARSE_BYTES: usize = 1 << 20;

/// A compiled grammar and highlight query, shared by every document using it.
pub struct Grammar {
    pub language: Language,
    pub query: Query,
    /// The ink of each capture of `query` (`None` for captures we do not color).
    inks: Vec<Option<Ink>>,
}

type Compiled = Result<Query, String>;

thread_local! {
    static GRAMMARS: RefCell<HashMap<&'static str, Rc<Grammar>>> = RefCell::new(HashMap::new());
    /// Highlight queries being compiled on worker threads.
    static PENDING: RefCell<HashMap<&'static str, std::thread::JoinHandle<Compiled>>> = RefCell::new(HashMap::new());
}

fn compile(lang: &'static Lang) -> Compiled {
    Query::new(&(lang.language)(), lang.highlights).map_err(|e| format!("{}: {e}", lang.id))
}

/// Starts compiling `lang`'s highlight query on a worker thread (it takes
/// several milliseconds), so it is ready by the first paint.
pub fn prefetch(lang: &'static Lang) {
    if GRAMMARS.with(|g| g.borrow().contains_key(lang.id)) {
        return;
    }
    PENDING.with(|p| {
        p.borrow_mut().entry(lang.id).or_insert_with(|| std::thread::spawn(move || compile(lang)));
    });
}

/// The grammar for `lang`, compiled on first use (or taken from `prefetch`).
pub fn grammar(lang: &'static Lang) -> Result<Rc<Grammar>, String> {
    if let Some(g) = GRAMMARS.with(|g| g.borrow().get(lang.id).cloned()) {
        return Ok(g);
    }
    let language = (lang.language)();
    let pending = PENDING.with(|p| p.borrow_mut().remove(lang.id));
    let query = match pending {
        Some(h) => h.join().unwrap_or_else(|_| compile(lang)),
        None => compile(lang),
    }?;
    let inks = query
        .capture_names()
        .iter()
        .map(|n| if *n == "none" { Some(Ink::Text) } else { Ink::from_name(n) })
        .collect();
    let g = Rc::new(Grammar {
        language,
        query,
        inks,
    });
    GRAMMARS.with(|m| m.borrow_mut().insert(lang.id, g.clone()));
    Ok(g)
}

fn point(p: Pos) -> Point {
    Point::new(p.line, p.col)
}

pub fn input_edit(c: &Change) -> InputEdit {
    InputEdit {
        start_byte: c.start,
        old_end_byte: c.old_end,
        new_end_byte: c.new_end,
        start_position: point(c.start_pos),
        old_end_position: point(c.old_end_pos),
        new_end_position: point(c.new_end_pos),
    }
}

/// Parses `rope`, reusing `old` (already edited to match) when given.
fn parse_rope(parser: &mut Parser, rope: &Rope, old: Option<&Tree>) -> Option<Tree> {
    let len = rope.len_bytes();
    parser.parse_with_options(
        &mut |byte: usize, _: Point| -> &[u8] {
            if byte >= len {
                return &[];
            }
            let (chunk, start, _, _) = rope.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - start..]
        },
        old,
        None,
    )
}

/// A colored byte range within one line.
pub type Span = (usize, usize, Ink);

pub struct Syntax {
    pub lang: &'static Lang,
    grammar: Rc<Grammar>,
    parser: Parser,
    tree: Option<Tree>,
    /// The tree does not match the text yet.
    dirty: bool,
    /// While a background parse runs: edits to replay onto its tree.
    pending: Option<Vec<InputEdit>>,
}

impl Syntax {
    pub fn new(lang: &'static Lang) -> Result<Syntax, String> {
        let grammar = grammar(lang)?;
        let mut parser = Parser::new();
        parser.set_language(&grammar.language).map_err(|e| e.to_string())?;
        Ok(Syntax {
            lang,
            grammar,
            parser,
            tree: None,
            dirty: true,
            pending: None,
        })
    }

    pub fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }

    /// The tree matches the text.
    pub fn is_current(&self) -> bool {
        self.tree.is_some() && !self.dirty
    }

    pub fn edit(&mut self, c: &Change) {
        let e = input_edit(c);
        if let Some(p) = &mut self.pending {
            p.push(e);
        } else if let Some(t) = &mut self.tree {
            t.edit(&e);
        }
        self.dirty = true;
    }

    /// The tree has edits it has not been reparsed for.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Brings the tree up to date (incrementally) unless a background parse is
    /// still running.
    pub fn parse(&mut self, rope: &Rope) {
        if !self.dirty || self.pending.is_some() {
            return;
        }
        if let Some(t) = parse_rope(&mut self.parser, rope, self.tree.as_ref()) {
            self.tree = Some(t);
        }
        self.dirty = false;
    }

    /// Starts the first parse on a worker thread; `done` runs there with the
    /// tree, and its owner hands the tree back through [`Syntax::finish_background`].
    pub fn start_background(&mut self, rope: Rope, done: impl FnOnce(Tree) + Send + 'static) {
        self.pending = Some(Vec::new());
        self.tree = None;
        let language = self.grammar.language.clone();
        std::thread::spawn(move || {
            let mut parser = Parser::new();
            if parser.set_language(&language).is_ok()
                && let Some(t) = parse_rope(&mut parser, &rope, None)
            {
                done(t);
            }
        });
    }

    pub fn parsing_in_background(&self) -> bool {
        self.pending.is_some()
    }

    /// Takes the tree from a background parse, replays the edits made since it
    /// started, and leaves an incremental reparse for the next paint.
    pub fn finish_background(&mut self, mut tree: Tree) {
        let Some(edits) = self.pending.take() else { return };
        for e in &edits {
            tree.edit(e);
        }
        self.tree = Some(tree);
        self.dirty = !edits.is_empty();
    }

    /// Highlight spans for `lines` (start byte of each line plus one past the
    /// last); one list per line, in byte columns, without plain-text runs.
    pub fn highlight(&self, rope: &Rope, line_starts: &[usize]) -> Vec<Vec<Span>> {
        let n = line_starts.len().saturating_sub(1);
        let mut out = vec![Vec::new(); n];
        let Some(tree) = &self.tree else { return out };
        if n == 0 {
            return out;
        }
        let (start, end) = (line_starts[0], line_starts[n]);
        let mut caps: Vec<(usize, usize, usize, Ink)> = Vec::new();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(start..end);
        let text = |node: Node| rope.byte_slice(node.byte_range()).chunks().map(str::as_bytes);
        let mut it = cursor.captures(&self.grammar.query, tree.root_node(), text);
        while let Some((m, i)) = it.next() {
            let c = m.captures[*i];
            if let Some(ink) = self.grammar.inks[c.index as usize] {
                caps.push((c.node.start_byte(), c.node.end_byte(), m.pattern_index, ink));
            }
        }
        // Wider captures first, so narrower ones paint over them; for equal
        // ranges, later patterns win.
        caps.sort_by_key(|&(s, e, p, _)| (std::cmp::Reverse(e - s), p));
        for (li, out_line) in out.iter_mut().enumerate() {
            let (ls, le) = (line_starts[li], line_starts[li + 1]);
            let len = (le - ls).min(MAX_RENDER_BYTES);
            let mut inks: Vec<Option<Ink>> = vec![None; len];
            for &(s, e, _, ink) in &caps {
                let (a, b) = (s.max(ls), e.min(ls + len));
                if a < b {
                    inks[a - ls..b - ls].fill(Some(ink));
                }
            }
            let mut i = 0;
            while i < len {
                let ink = inks[i];
                let mut j = i + 1;
                while j < len && inks[j] == ink {
                    j += 1;
                }
                if let Some(ink) = ink
                    && ink != Ink::Text
                {
                    out_line.push((i, j, ink));
                }
                i = j;
            }
        }
        out
    }
}

/// What kind of text an offset is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Context {
    Code,
    String,
    Comment,
}

impl Syntax {
    /// Whether `off` is inside a string literal or comment. A line comment
    /// still holds the end of its line.
    pub fn context_at(&self, off: usize) -> Context {
        let Some(tree) = &self.tree else { return Context::Code };
        let root = tree.root_node();
        let Some(mut node) = root.descendant_for_byte_range(off.saturating_sub(1), off) else {
            return Context::Code;
        };
        loop {
            let k = node.kind();
            if self.lang.comment_nodes.contains(&k) && node.start_byte() < off && off <= node.end_byte() {
                return Context::Comment;
            }
            if self.lang.string_nodes.contains(&k) && node.start_byte() < off && off < node.end_byte() {
                return Context::String;
            }
            match node.parent() {
                Some(p) => node = p,
                None => return Context::Code,
            }
        }
    }

    /// The bracket matching the one at byte `at` (a bracket token in the
    /// tree, so brackets in strings and comments never match).
    pub fn matching_bracket(&self, at: usize, brackets: &[(char, char)]) -> Option<usize> {
        let tree = self.tree.as_ref()?;
        let node = tree.root_node().descendant_for_byte_range(at, at + 1)?;
        if node.start_byte() != at || node.child_count() != 0 {
            return None;
        }
        let k = node.kind();
        let parent = node.parent()?;
        let mut c = parent.walk();
        let kids: Vec<Node> = parent.children(&mut c).collect();
        let i = kids.iter().position(|n| n.id() == node.id())?;
        for &(o, cl) in brackets {
            let (o, cl) = (o.to_string(), cl.to_string());
            if k == o {
                return kids[i + 1..].iter().find(|n| n.kind() == cl).map(|n| n.start_byte());
            }
            if k == cl {
                return kids[..i].iter().rev().find(|n| n.kind() == o).map(|n| n.start_byte());
            }
        }
        None
    }

    /// Fold regions (header line, last folded line) whose header is in
    /// `first..=last`, from the language's fold nodes. A bracketed region keeps
    /// a line holding only its closer visible.
    pub fn fold_ranges(&self, rope: &Rope, first: usize, last: usize) -> Vec<(usize, usize)> {
        let Some(tree) = &self.tree else { return Vec::new() };
        let mut out: Vec<(usize, usize)> = Vec::new();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            let (sr, er) = (node.start_position().row, node.end_position().row);
            if er < first || sr > last + 1 {
                continue;
            }
            if let Some(&(_, from_parent)) = self.lang.fold_nodes.iter().find(|f| f.0 == node.kind()) {
                let header = if from_parent { node.parent().map_or(sr, |p| p.start_position().row) } else { sr };
                let mut end = er;
                if node.end_position().column == 0 && end > header {
                    end -= 1;
                }
                if !from_parent && end > header + 1 {
                    let line = rope.line(end);
                    let closer = line.chars().find(|c| !c.is_whitespace());
                    if closer.is_some_and(|c| ")]}".contains(c)) {
                        end -= 1;
                    }
                }
                if end > header && (first..=last).contains(&header) {
                    match out.iter_mut().find(|r| r.0 == header) {
                        Some(r) => r.1 = r.1.max(end),
                        None => out.push((header, end)),
                    }
                }
            }
            let mut c = node.walk();
            stack.extend(node.children(&mut c));
        }
        out.sort_unstable();
        out
    }

    /// The smallest node range strictly larger than `start..end` that holds it.
    pub fn expand(&self, start: usize, end: usize) -> Option<(usize, usize)> {
        let tree = self.tree.as_ref()?;
        let mut node = tree.root_node().descendant_for_byte_range(start, end)?;
        loop {
            let (a, e) = (node.start_byte(), node.end_byte());
            if a <= start && e >= end && (a, e) != (start, end) {
                return Some((a, e));
            }
            node = node.parent()?;
        }
    }

    /// The innermost bracket pair around `off`: (open, close) offsets.
    pub fn enclosing_brackets(&self, off: usize, brackets: &[(char, char)]) -> Option<(usize, usize)> {
        let tree = self.tree.as_ref()?;
        let mut node = tree.root_node().descendant_for_byte_range(off, off)?;
        loop {
            let mut c = node.walk();
            let kids: Vec<Node> = node.children(&mut c).collect();
            for &(o, cl) in brackets {
                let (o, cl) = (o.to_string(), cl.to_string());
                if let (Some(a), Some(b)) = (
                    kids.iter().find(|n| n.kind() == o && n.start_byte() < off),
                    kids.iter().rev().find(|n| n.kind() == cl && n.start_byte() >= off),
                ) {
                    return Some((a.start_byte(), b.start_byte()));
                }
            }
            node = node.parent()?;
        }
    }
}

/// Bracket matching by scanning text (for plain text): the bracket matching
/// the one at `at`, within `limit` bytes.
pub fn scan_bracket(text: &Rope, at: usize, brackets: &[(char, char)], limit: usize) -> Option<usize> {
    if at >= text.len_bytes() {
        return None;
    }
    let c = text.char(text.byte_to_char(at));
    let (&(o, cl), forward) = brackets
        .iter()
        .find_map(|p| if p.0 == c { Some((p, true)) } else if p.1 == c { Some((p, false)) } else { None })?;
    let mut depth = 0usize;
    if forward {
        let mut off = at;
        for ch in text.byte_slice(at..(at + limit).min(text.len_bytes())).chars() {
            if ch == o {
                depth += 1;
            } else if ch == cl {
                depth -= 1;
                if depth == 0 {
                    return Some(off);
                }
            }
            off += ch.len_utf8();
        }
    } else {
        let start = at.saturating_sub(limit);
        let start = text.char_to_byte(text.byte_to_char(start));
        let mut off = at + c.len_utf8();
        let slice = text.byte_slice(start..at + c.len_utf8());
        for ch in slice.chars_at(slice.len_chars()).reversed() {
            off -= ch.len_utf8();
            if ch == cl {
                depth += 1;
            } else if ch == o {
                depth -= 1;
                if depth == 0 {
                    return Some(off);
                }
            }
        }
    }
    None
}

/// Highlight spans of the visible lines, kept until the text or range changes.
#[derive(Default)]
pub struct HighlightCache {
    pub version: u64,
    pub first: usize,
    pub lines: Vec<Vec<Span>>,
}

impl HighlightCache {
    pub fn get(&self, version: u64, line: usize) -> Option<&[Span]> {
        (self.version == version && line >= self.first)
            .then(|| self.lines.get(line - self.first).map(Vec::as_slice))
            .flatten()
    }

    pub fn covers(&self, version: u64, lines: &std::ops::Range<usize>) -> bool {
        self.version == version && lines.start >= self.first && lines.end <= self.first + self.lines.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Buffer;

    /// `text` highlighted as `lang`, written as `token:ink` for every colored run.
    #[allow(dead_code)]
    fn tokens(lang: &str, text: &str) -> String {
        let lang = crate::lang::by_id(lang).unwrap();
        let mut s = Syntax::new(lang).unwrap();
        let b = Buffer::new(text);
        s.parse(b.rope());
        let starts: Vec<usize> = (0..=b.line_count()).map(|l| b.line_start(l)).collect();
        let mut out = Vec::new();
        for (l, spans) in s.highlight(b.rope(), &starts).iter().enumerate() {
            let line = b.line(l);
            for &(a, e, ink) in spans {
                out.push(format!("{}:{ink:?}", &line[a..e]));
            }
        }
        out.join(" ")
    }

    #[test]
    fn every_registered_query_compiles() {
        for lang in crate::lang::LANGS {
            grammar(lang).unwrap();
        }
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_tokens() {
        let src = "@dataclass\nclass Point(Base):\n    def length(self, n: int = 2) -> float:\n        \"\"\"Doc.\"\"\"\n        return math.sqrt(self.x ** n) if n else None  # hi\n";
        assert_eq!(
            tokens("python", src),
            "@dataclass:Decorator \
             class:Keyword Point:Type Base:Type \
             def:Keyword length:Function self:VariableBuiltin n:Parameter int:Type =:Operator 2:Number ->:Operator float:Type \
             \"\"\"Doc.\"\"\":String \
             return:Control sqrt:Function self:VariableBuiltin x:Property **:Operator if:Control else:Control None:Constant # hi:Comment"
        );
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_fstring_braces_are_code() {
        assert_eq!(
            tokens("python", "f\"a{len(x)}\\n\"\n"),
            "f\"a:String {:Punctuation len:Function }:Punctuation \\n:Escape \":String"
        );
    }

    #[cfg(feature = "lang-sql")]
    #[test]
    fn sql_tokens() {
        let src = "-- totals\nSELECT name, COUNT(*) AS n FROM users WHERE id = 42 AND note = 'x' OR flag IS NULL;\n";
        assert_eq!(
            tokens("sql", src),
            "-- totals:Comment \
             SELECT:Keyword COUNT:Function *:Operator AS:Keyword FROM:Keyword users:Type WHERE:Keyword =:Operator 42:Number AND:Keyword =:Operator 'x':String OR:Keyword IS:Keyword NULL:Constant"
        );
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn incremental_edits_and_background_parse() {
        let lang = crate::lang::by_id("python").unwrap();
        let mut b = Buffer::new("x = 1\n");
        let mut s = Syntax::new(lang).unwrap();
        s.parse(b.rope());
        let tx = crate::text::Transaction::new(vec![crate::text::Edit::insert(0, "def f():\n    ")]);
        for c in b.apply(&tx).1 {
            s.edit(&c);
        }
        assert!(!s.is_current());
        s.parse(b.rope());
        assert!(s.is_current());
        assert!(s.tree().unwrap().root_node().to_sexp().contains("function_definition"));

        // A background parse with an edit made while it ran.
        let (tx_send, rx) = std::sync::mpsc::channel();
        s.start_background(b.rope().clone(), move |t| tx_send.send(t).unwrap());
        let tx2 = crate::text::Transaction::new(vec![crate::text::Edit::insert(b.len(), "y = 2\n")]);
        for c in b.apply(&tx2).1 {
            s.edit(&c);
        }
        let t = rx.recv().unwrap();
        s.finish_background(t);
        s.parse(b.rope());
        assert_eq!(s.tree().unwrap().root_node().end_byte(), b.len());
    }
}


