//! Formats queries: each clause on its own line with its body indented
//! under it, one select item per line, a line per JOIN and per top-level
//! AND or OR. Subqueries too long for their line get lines of their own.
//!
//! Only line breaks and indentation change. Spacing within a line, keyword
//! case and comments stay as written, and a statement with a syntax error is
//! left alone.

use rowan::{NodeOrToken, TextRange};

use crate::edit::{EditStyle, Edits, Piece, Rewrite, line_indent, span};
use crate::{SyntaxKind, SyntaxNode, SyntaxToken, parse};

/// The widest a list (GROUP BY, ORDER BY, VALUES) or CASE stays on one line.
const LIST_WIDTH: usize = 80;
/// The widest a subquery stays on the line of the expression it is in.
const SUBQUERY_WIDTH: usize = 50;

/// Formats the statements of `text`, or with a selection, the query or
/// statements it covers.
pub fn format_sql(text: &str, selection: Option<TextRange>, style: &EditStyle) -> Result<Rewrite, String> {
    let parse = parse(text);
    let root = parse.syntax();
    let targets: Vec<SyntaxNode> = match selection.filter(|s| !s.is_empty()) {
        Some(selection) => {
            let trimmed = trim(text, selection);
            let covering = match root.covering_element(trimmed) {
                NodeOrToken::Node(n) => Some(n),
                NodeOrToken::Token(t) => t.parent(),
            };
            let query = std::iter::successors(covering, SyntaxNode::parent)
                .take_while(|n| n.kind() != SyntaxKind::SOURCE_FILE)
                .find(|n| n.kind() == SyntaxKind::QUERY && n.text_range() == trimmed);
            match query {
                Some(query) => vec![query],
                None => root
                    .children()
                    .filter(|s| s.text_range().intersect(selection).is_some_and(|r| !r.is_empty() || s.text_range().contains_range(selection)))
                    .collect(),
            }
        }
        None => root.children().collect(),
    };
    let mut edits = Edits::default();
    let mut skipped = 0;
    let mut formatted = 0;
    for target in targets {
        let range = target.text_range();
        if parse.errors().iter().any(|e| range.contains_range(e.range) || e.range.contains_range(range)) {
            skipped += 1;
            continue;
        }
        if matches!(target.kind(), SyntaxKind::UNKNOWN_STMT | SyntaxKind::ERROR) {
            continue;
        }
        let trimmed = trim(text, range);
        let base = line_indent(text, usize::from(trimmed.start())).to_string();
        let mut printer = Printer::new(base, &style.indent);
        printer.node(&target);
        let new = printer.out;
        if new != text[span(trimmed)] {
            edits.replace(trimmed, Piece::new(new));
        }
        formatted += 1;
    }
    if formatted == 0 && skipped > 0 {
        return Err("The SQL has a syntax error here, so it was left as it is".into());
    }
    Ok(edits.apply(text))
}

/// `range` without whitespace at its ends.
fn trim(text: &str, range: TextRange) -> TextRange {
    let s = &text[span(range)];
    let start = usize::from(range.start()) + (s.len() - s.trim_start().len());
    let end = usize::from(range.end()) - (s.len() - s.trim_end().len());
    TextRange::new((start as u32).into(), (end.max(start) as u32).into())
}

struct Printer<'a> {
    out: String,
    /// Put before every line after the first.
    base: String,
    unit: &'a str,
    level: usize,
    /// A line break before the next token, where the layout puts one.
    want_line: bool,
    /// A line break the text needs, after a line comment.
    must_break: bool,
    /// The text had a blank line where the next break goes.
    blank: bool,
    /// Lay everything out on one line (a short subquery).
    flat: bool,
    /// A line comment was seen while laying out flat.
    broke: bool,
}

fn is_set_op_keyword(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::UNION_KW | SyntaxKind::EXCEPT_KW | SyntaxKind::INTERSECT_KW)
}

impl<'a> Printer<'a> {
    fn new(base: String, unit: &'a str) -> Self {
        Printer {
            out: String::new(),
            base,
            unit,
            level: 0,
            want_line: false,
            must_break: false,
            blank: false,
            flat: false,
            broke: false,
        }
    }

    fn newline(&mut self) {
        if !self.flat && !self.out.is_empty() {
            self.want_line = true;
        }
    }

    fn at_line_start(&self) -> bool {
        self.out.is_empty() || self.out.ends_with('\n') || self.out.trim_end_matches([' ', '\t']).ends_with('\n')
    }

    fn break_line(&mut self) {
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        self.out.push('\n');
        if std::mem::take(&mut self.blank) {
            self.out.push('\n');
        }
        self.out.push_str(&self.base);
        for _ in 0..self.level {
            self.out.push_str(self.unit);
        }
        self.want_line = false;
        self.must_break = false;
    }

    fn token(&mut self, token: &SyntaxToken) {
        let kind = token.kind();
        match kind {
            SyntaxKind::WHITESPACE => {
                if token.text().matches('\n').count() >= 2 {
                    self.blank = true;
                }
                return;
            }
            SyntaxKind::LINE_COMMENT => {
                if self.flat {
                    self.broke = true;
                }
                let own_line = token.prev_token().is_none_or(|p| p.kind() == SyntaxKind::WHITESPACE && p.text().contains('\n'));
                if own_line {
                    self.newline();
                }
                self.emit(token, true);
                self.must_break = true;
                return;
            }
            _ => {}
        }
        if self.out.is_empty() {
            self.blank = false;
        }
        let had_space = token.prev_token().is_some_and(|p| p.kind().is_trivia());
        self.emit(token, had_space);
        self.blank = false;
    }

    fn emit(&mut self, token: &SyntaxToken, had_space: bool) {
        if (self.want_line || self.must_break) && !self.out.is_empty() {
            self.break_line();
        } else if !self.at_line_start() {
            let prev = self.out.chars().last();
            let next = token.kind();
            let tight_after = matches!(prev, Some('(' | '[' | '.'));
            let tight_before = matches!(
                next,
                SyntaxKind::COMMA | SyntaxKind::R_PAREN | SyntaxKind::R_BRACK | SyntaxKind::SEMICOLON | SyntaxKind::DOT | SyntaxKind::DOUBLE_COLON
            ) || self.out.ends_with("::");
            if prev == Some(',') || (had_space && !tight_after && !tight_before) {
                self.out.push(' ');
            }
        }
        self.out.push_str(token.text());
    }

    fn element(&mut self, element: NodeOrToken<SyntaxNode, SyntaxToken>) {
        match element {
            NodeOrToken::Node(n) => self.node(&n),
            NodeOrToken::Token(t) => self.token(&t),
        }
    }

    fn node(&mut self, node: &SyntaxNode) {
        match node.kind() {
            SyntaxKind::QUERY => self.query(node),
            SyntaxKind::WITH_CLAUSE => self.with(node),
            SyntaxKind::CTE => self.cte(node),
            SyntaxKind::SELECT_CORE => {
                for element in node.children_with_tokens() {
                    if element.as_node().is_some() {
                        self.newline();
                    }
                    self.element(element);
                }
            }
            SyntaxKind::SELECT_CLAUSE => self.select_clause(node),
            SyntaxKind::JOIN_EXPR => self.join(node),
            SyntaxKind::SET_OP => self.set_op(node),
            SyntaxKind::WHERE_CLAUSE | SyntaxKind::HAVING_CLAUSE | SyntaxKind::QUALIFY_CLAUSE => self.condition(node),
            SyntaxKind::GROUP_BY_CLAUSE | SyntaxKind::ORDER_BY_CLAUSE | SyntaxKind::VALUES_CLAUSE => {
                if self.fits(node, LIST_WIDTH) {
                    self.plain(node);
                } else {
                    self.list(node);
                }
            }
            SyntaxKind::CASE_EXPR if !self.fits(node, LIST_WIDTH) => self.case(node),
            _ => self.plain(node),
        }
    }

    /// The node's elements in order; queries inside get lines of their own.
    fn plain(&mut self, node: &SyntaxNode) {
        let mut after_paren = false;
        for element in node.children_with_tokens() {
            match &element {
                NodeOrToken::Node(n) if n.kind() == SyntaxKind::QUERY => {
                    if after_paren {
                        self.subquery(n);
                    } else {
                        self.newline();
                        self.query(n);
                    }
                }
                _ => {
                    if !element.kind().is_trivia() {
                        after_paren = element.kind() == SyntaxKind::L_PAREN;
                    }
                    self.element(element);
                }
            }
        }
    }

    /// Whether the node fits on one line of `width` from where the line is now.
    fn fits(&self, node: &SyntaxNode, width: usize) -> bool {
        if self.flat {
            return true;
        }
        let mut flat = Printer::new(String::new(), self.unit);
        flat.flat = true;
        flat.node(node);
        !flat.broke && !flat.out.contains('\n') && flat.out.len() <= width
    }

    fn subquery(&mut self, query: &SyntaxNode) {
        let nested = query.descendants().skip(1).any(|n| n.kind() == SyntaxKind::QUERY);
        if self.flat || (!nested && self.fits(query, SUBQUERY_WIDTH)) {
            let flat = std::mem::replace(&mut self.flat, true);
            self.query(query);
            self.flat = flat;
            return;
        }
        self.level += 1;
        self.newline();
        self.query(query);
        self.level -= 1;
        self.newline();
    }

    fn query(&mut self, node: &SyntaxNode) {
        for element in node.children_with_tokens() {
            if element.as_node().is_some() {
                self.newline();
            }
            self.element(element);
        }
    }

    fn with(&mut self, node: &SyntaxNode) {
        let mut after_comma = false;
        for element in node.children_with_tokens() {
            if element.kind() == SyntaxKind::CTE && after_comma {
                self.newline();
            }
            if !element.kind().is_trivia() {
                after_comma = element.kind() == SyntaxKind::COMMA;
            }
            self.element(element);
        }
    }

    fn cte(&mut self, node: &SyntaxNode) {
        for element in node.children_with_tokens() {
            match &element {
                NodeOrToken::Node(n) if n.kind() == SyntaxKind::QUERY => {
                    if self.flat {
                        self.query(n);
                        continue;
                    }
                    self.level += 1;
                    self.newline();
                    self.query(n);
                    self.level -= 1;
                    self.newline();
                }
                _ => self.element(element),
            }
        }
    }

    /// `SELECT [DISTINCT]`, then one item per line under it.
    fn select_clause(&mut self, node: &SyntaxNode) {
        let mut indented = false;
        for element in node.children_with_tokens() {
            if element.kind() == SyntaxKind::SELECT_ITEM {
                if !indented {
                    self.level += 1;
                    indented = true;
                }
                self.newline();
            }
            self.element(element);
        }
        if indented {
            self.level -= 1;
        }
    }

    /// The left side, then the JOIN keywords on a line of their own.
    fn join(&mut self, node: &SyntaxNode) {
        // A join in parentheses stays on its line.
        let parenthesized = node
            .ancestors()
            .skip(1)
            .take_while(|a| a.kind() != SyntaxKind::FROM_CLAUSE)
            .any(|a| a.kind() == SyntaxKind::PAREN_JOIN);
        if parenthesized {
            return self.plain(node);
        }
        let mut seen_left = false;
        let mut joined = false;
        for element in node.children_with_tokens() {
            let kind = element.kind();
            if seen_left && !joined && (kind.is_keyword() || kind == SyntaxKind::COMMA) {
                self.newline();
                joined = true;
            }
            if element.as_node().is_some() {
                seen_left = true;
            }
            self.element(element);
        }
    }

    fn set_op(&mut self, node: &SyntaxNode) {
        let mut after_keyword = false;
        for element in node.children_with_tokens() {
            let kind = element.kind();
            if is_set_op_keyword(kind) {
                self.newline();
                after_keyword = true;
            } else if element.as_node().is_some() && after_keyword {
                self.newline();
                after_keyword = false;
            }
            self.element(element);
        }
    }

    /// WHERE and its kin: a top-level AND (or OR) chain breaks before each
    /// operator, one level in.
    fn condition(&mut self, node: &SyntaxNode) {
        for element in node.children_with_tokens() {
            match &element {
                NodeOrToken::Node(n) if n.kind() == SyntaxKind::BIN_EXPR && chain_operator(n).is_some() => {
                    let op = chain_operator(n).unwrap();
                    self.level += 1;
                    self.chain(n, op);
                    self.level -= 1;
                }
                _ => self.element(element),
            }
        }
    }

    fn chain(&mut self, node: &SyntaxNode, op: SyntaxKind) {
        for element in node.children_with_tokens() {
            match &element {
                NodeOrToken::Node(n) if n.kind() == SyntaxKind::BIN_EXPR && chain_operator(n) == Some(op) => {
                    self.chain(n, op);
                }
                NodeOrToken::Token(t) if t.kind() == op => {
                    self.newline();
                    self.token(t);
                }
                _ => self.element(element),
            }
        }
    }

    /// GROUP BY, ORDER BY, VALUES: one item per line under the keywords.
    fn list(&mut self, node: &SyntaxNode) {
        let mut indented = false;
        for element in node.children_with_tokens() {
            if element.as_node().is_some() {
                if !indented {
                    self.level += 1;
                    indented = true;
                }
                self.newline();
            }
            self.element(element);
        }
        if indented {
            self.level -= 1;
        }
    }

    fn case(&mut self, node: &SyntaxNode) {
        for element in node.children_with_tokens() {
            match element.kind() {
                SyntaxKind::WHEN_CLAUSE | SyntaxKind::ELSE_CLAUSE => {
                    self.level += 1;
                    self.newline();
                    self.element(element);
                    self.level -= 1;
                }
                SyntaxKind::END_KW => {
                    self.newline();
                    self.element(element);
                }
                _ => self.element(element),
            }
        }
    }
}

/// The operator of a binary AND or OR.
fn chain_operator(node: &SyntaxNode) -> Option<SyntaxKind> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .map(|t| t.kind())
        .find(|k| matches!(*k, SyntaxKind::AND_KW | SyntaxKind::OR_KW))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(text: &str) -> String {
        format_sql(text, None, &EditStyle::default()).unwrap().text
    }

    #[test]
    fn clauses_get_lines_and_items_indent() {
        assert_eq!(fmt("select * from abc"), "select\n    *\nfrom abc");
        assert_eq!(
            fmt("with cte_name as (select * from abc) select * from cte_name"),
            "with cte_name as (\n    select\n        *\n    from abc\n)\nselect\n    *\nfrom cte_name"
        );
        assert_eq!(
            fmt("SELECT a, sum(b) AS total FROM t JOIN u ON t.id = u.id LEFT JOIN v USING (id) WHERE a > 1 AND b < 2 GROUP BY a ORDER BY total DESC LIMIT 10"),
            "SELECT\n    a,\n    sum(b) AS total\nFROM t\nJOIN u ON t.id = u.id\nLEFT JOIN v USING (id)\nWHERE a > 1\n    AND b < 2\nGROUP BY a\nORDER BY total DESC\nLIMIT 10"
        );
    }

    #[test]
    fn short_subqueries_stay_inline_and_long_ones_break() {
        assert_eq!(
            fmt("select * from t where id in (select id from u)"),
            "select\n    *\nfrom t\nwhere id in (select id from u)"
        );
        assert_eq!(
            fmt("select * from (select a, b, c, d from some_long_table_name where a > 1) s"),
            "select\n    *\nfrom (\n    select\n        a,\n        b,\n        c,\n        d\n    from some_long_table_name\n    where a > 1\n) s"
        );
    }

    #[test]
    fn comments_and_blank_lines_stay() {
        assert_eq!(
            fmt("select a, -- the key\n  b\nfrom t\n\n-- done\nwhere x"),
            "select\n    a, -- the key\n    b\nfrom t\n\n-- done\nwhere x"
        );
    }

    #[test]
    fn statements_format_separately_and_errors_are_left_alone() {
        assert_eq!(fmt("select 1 from t;\nselect from where;\n"), "select\n    1\nfrom t;\nselect from where;\n");
        assert_eq!(fmt("select a from t union all select b from u"), "select\n    a\nfrom t\nunion all\nselect\n    b\nfrom u");
    }

    #[test]
    fn a_selected_subquery_formats_alone() {
        let text = "select * from (select a, b from t) s";
        let start = text.find("select a").unwrap();
        let range = TextRange::new((start as u32).into(), ((start + 18) as u32).into());
        let rewrite = format_sql(text, Some(range), &EditStyle::default()).unwrap();
        assert_eq!(rewrite.text, "select * from (select\n    a,\n    b\nfrom t) s");
    }
}
