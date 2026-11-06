//! A recursive-descent parser for rule text. It reports one problem per rule
//! and skips to the next `;`, so later rules still parse.
//!
//! The parser also tells completion what fits where the text ends: every
//! place it looks for a token at the end of the text is recorded as an
//! [`Expect`], until the first one it fails on. Completion parses the text
//! up to the caret and offers those.

use crate::ast::*;
use crate::catalog::{self, Form};
use crate::lexer::{Token, TokenKind, tokenize, unquote};
use crate::{Diagnostic, Severity};

/// Something that could come next where the text ends.
#[derive(Clone, Debug, PartialEq)]
pub enum Expect {
    Keyword(&'static str),
    Symbol(&'static str),
    /// The start of a value: `c`, a column, a function, a literal.
    Expr,
    /// A comparison or a test like `is null` after a value.
    Operator,
    /// The start of formatting: `bg:`, `bold`, `bar(…)`.
    Action,
    /// Value number `index` after `property:`.
    Value { property: String, index: usize },
    /// Argument number `index` of a call. `action` says it's a formatting
    /// call like `bar(…)` rather than a function in a condition.
    Arg { call: String, index: usize, action: bool },
    /// The value after `name=` in a formatting call.
    NamedValue { call: String, name: String },
    /// A field name after `.`, below what `path` reaches.
    Field(FieldPath),
}

/// Where a `.` is: below the cell or a column, through fields and list items.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldPath {
    /// The column; `None` is the cell.
    pub column: Option<String>,
    /// `Some(name)` for a field, `None` for a list item.
    pub steps: Vec<Option<String>>,
}

impl FieldPath {
    fn of(expr: &Expr) -> Option<FieldPath> {
        match &expr.kind {
            ExprKind::Cell => Some(FieldPath { column: None, steps: Vec::new() }),
            ExprKind::Column(name) => Some(FieldPath { column: Some(name.clone()), steps: Vec::new() }),
            ExprKind::Field { expr, name, .. } => {
                let mut path = FieldPath::of(expr)?;
                path.steps.push(Some(name.clone()));
                Some(path)
            }
            ExprKind::Index { expr, .. } => {
                let mut path = FieldPath::of(expr)?;
                path.steps.push(None);
                Some(path)
            }
            _ => None,
        }
    }
}

pub struct Parsed {
    pub rules: Rules,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn parse(text: &str) -> Parsed {
    let (rules, diagnostics, _) = run(text, tokenize(text));
    Parsed { rules, diagnostics }
}

/// Parses `text` as one expression, such as a tooltip template's field.
pub(crate) fn parse_expression(text: &str) -> (Expr, Vec<Diagnostic>) {
    let tokens = tokenize(text).into_iter().filter(|t| t.kind != TokenKind::Comment).collect();
    let mut p = Parser {
        text,
        tokens,
        pos: 0,
        diagnostics: Vec::new(),
        expected: Vec::new(),
        frozen: false,
        rule_failed: false,
    };
    let expr = p.expr();
    if !p.eof() {
        let found = p.found();
        p.error(format!("expected `:`, `|` or `}}` after the field's value, found {found}"));
    }
    (expr, p.diagnostics)
}

/// What could follow `tokens`, which end at the caret.
pub(crate) fn expected(text: &str, tokens: Vec<Token>) -> Vec<Expect> {
    run(text, tokens).2
}

fn run(text: &str, tokens: Vec<Token>) -> (Rules, Vec<Diagnostic>, Vec<Expect>) {
    let tokens = tokens.into_iter().filter(|t| t.kind != TokenKind::Comment).collect();
    let mut p = Parser {
        text,
        tokens,
        pos: 0,
        diagnostics: Vec::new(),
        expected: Vec::new(),
        frozen: false,
        rule_failed: false,
    };
    let rules = p.rules();
    (rules, p.diagnostics, p.expected)
}

struct Parser<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
    expected: Vec<Expect>,
    /// Set by the first error at the end: nothing after it is a real option.
    frozen: bool,
    /// The current rule already has its error.
    rule_failed: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn kind(&self) -> Option<TokenKind> {
        self.peek().map(|t| t.kind)
    }

    fn kind_at(&self, n: usize) -> Option<TokenKind> {
        self.tokens.get(self.pos + n).map(|t| t.kind)
    }

    fn eof(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn token_text(&self) -> &str {
        self.peek().map_or("", |t| t.text(self.text))
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        self.pos += 1;
        token
    }

    /// Where the last consumed token ends.
    fn prev_end(&self) -> usize {
        self.pos.checked_sub(1).map_or(0, |i| self.tokens[i].span.end)
    }

    fn here(&self) -> usize {
        self.peek().map_or(self.prev_end(), |t| t.span.start)
    }

    fn note(&mut self, expect: Expect) {
        if self.eof() && !self.frozen && !self.expected.contains(&expect) {
            self.expected.push(expect);
        }
    }

    fn at(&mut self, kind: TokenKind) -> bool {
        if self.eof() {
            if let Some(symbol) = symbol(kind) {
                self.note(Expect::Symbol(symbol));
            }
            return false;
        }
        self.kind() == Some(kind)
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        let found = self.at(kind);
        if found {
            self.bump();
        }
        found
    }

    fn at_kw(&mut self, keyword: &'static str) -> bool {
        if self.eof() {
            self.note(Expect::Keyword(keyword));
            return false;
        }
        self.kind() == Some(TokenKind::Ident) && self.token_text().eq_ignore_ascii_case(keyword)
    }

    fn eat_kw(&mut self, keyword: &'static str) -> bool {
        let found = self.at_kw(keyword);
        if found {
            self.bump();
        }
        found
    }

    /// Whether the next token is a keyword, without noting anything.
    fn is_kw(&self, keyword: &str) -> bool {
        self.kind() == Some(TokenKind::Ident) && self.token_text().eq_ignore_ascii_case(keyword)
    }

    /// The next token for a message: `` `then` `` or "the end of the rules".
    fn found(&self) -> String {
        match self.peek() {
            None => "the end of the rules".into(),
            Some(t) if t.kind == TokenKind::Unknown => format!("the character `{}`", t.text(self.text)),
            Some(t) => {
                let text = t.text(self.text);
                let short: String = text.chars().take(24).collect();
                format!("`{short}`{}", if short.len() < text.len() { "…" } else { "" })
            }
        }
    }

    /// Reports a problem at the next token (or the end), once per rule.
    fn error(&mut self, message: impl Into<String>) {
        let span = match self.peek() {
            Some(t) => t.span.clone(),
            None => {
                self.frozen = true;
                let end = self.prev_end();
                end..end
            }
        };
        self.error_at(span, message);
    }

    fn error_at(&mut self, span: Span, message: impl Into<String>) {
        if self.eof() {
            self.frozen = true;
        }
        if !self.rule_failed {
            self.rule_failed = true;
            self.diagnostics.push(Diagnostic { span, message: message.into(), severity: Severity::Error });
        }
    }

    fn rules(&mut self) -> Rules {
        let mut rules = Vec::new();
        let mut separated = true;
        loop {
            while self.kind() == Some(TokenKind::Semi) {
                self.bump();
                separated = true;
            }
            if self.eof() {
                if separated {
                    self.note(Expect::Keyword("if"));
                    self.note(Expect::Action);
                }
                break;
            }
            separated = false;
            self.rule_failed = false;
            let start = self.pos;
            if let Some(rule) = self.rule() {
                rules.push(rule);
            }
            if self.eat(TokenKind::Semi) {
                separated = true;
                continue;
            }
            if self.eof() {
                continue;
            }
            if self.is_kw("if") && !self.rule_failed {
                self.error("expected `;` before the next rule: rules are separated by `;`");
                continue;
            }
            if self.kind() == Some(TokenKind::Ident) && !self.rule_failed {
                self.error(format!("expected `,` before {} or `;` to end the rule", self.found()));
            } else {
                self.error(format!("expected `;` to end the rule, found {}", self.found()));
            }
            // Skip what's left of the rule, always moving forward.
            if self.pos == start {
                self.bump();
            }
            while !self.eof() && self.kind() != Some(TokenKind::Semi) {
                self.bump();
            }
        }
        Rules { rules }
    }

    fn rule(&mut self) -> Option<Rule> {
        let start = self.here();
        let mut branches = Vec::new();
        let mut otherwise = None;
        if self.eat_kw("if") {
            branches.push(self.branch());
            if !self.rule_failed {
                while self.eat_kw("elif") {
                    branches.push(self.branch());
                    if self.rule_failed {
                        break;
                    }
                }
            }
            if !self.rule_failed && self.eat_kw("else") {
                otherwise = Some(self.actions());
            }
        } else {
            if self.is_kw("then") || self.is_kw("else") || self.is_kw("elif") {
                self.error(format!("expected `if` before {}", self.found()));
                return None;
            }
            let actions = self.actions();
            if actions.is_empty() {
                return None;
            }
            branches.push(Branch { condition: None, actions });
        }
        Some(Rule { branches, otherwise, span: start..self.prev_end().max(start) })
    }

    /// `<condition> then <actions>`, after `if` or `elif`.
    fn branch(&mut self) -> Branch {
        let condition = self.expr();
        if !self.eat_kw("then") && !self.rule_failed {
            if self.at_action() {
                self.error(format!("expected `then` before {}", self.found()));
            } else if self.eof() {
                self.error("expected `then` and the formatting after the condition");
            } else {
                self.error(format!("expected `then` after the condition, found {}", self.found()));
                return Branch { condition: Some(condition), actions: Vec::new() };
            }
        }
        let actions = if self.rule_failed && !self.at_action() { Vec::new() } else { self.actions() };
        Branch { condition: Some(condition), actions }
    }

    /// Whether the next token starts formatting (for a forgotten `then`).
    fn at_action(&self) -> bool {
        self.kind() == Some(TokenKind::Ident) && catalog::action(self.token_text()).is_some()
    }

    fn actions(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        while let Some(action) = self.action() {
            actions.push(action);
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        actions
    }

    fn action(&mut self) -> Option<Action> {
        if self.eof() {
            self.note(Expect::Action);
            self.error("expected formatting, like `bg: red` or `bold`");
            return None;
        }
        if self.kind() != Some(TokenKind::Ident) || catalog::is_reserved(self.token_text()) {
            self.error(format!("expected formatting, like `bg: red` or `bold`, found {}", self.found()));
            return None;
        }
        let name_token = self.bump();
        let name = name_token.text(self.text).to_ascii_lowercase();
        let def = catalog::action(&name);
        let kind = if self.kind() == Some(TokenKind::Colon) {
            self.bump();
            ActionKind::Property(self.values(&name))
        } else if self.kind() == Some(TokenKind::LParen) {
            self.bump();
            ActionKind::Call(self.args(&name, true))
        } else if let Some(Form::Property(_)) = def.map(|d| d.form) {
            if self.starts_value() {
                self.error(format!("expected `:` after `{name}`"));
                ActionKind::Property(self.values(&name))
            } else {
                self.at(TokenKind::Colon);
                let signature = def.map_or("", |d| d.signature);
                self.error_at(name_token.span.clone(), format!("`{name}` needs a value: {signature}"));
                ActionKind::Property(Vec::new())
            }
        } else {
            if def.is_some_and(|d| d.form == Form::Call) {
                self.at(TokenKind::LParen);
            }
            ActionKind::Flag
        };
        Some(Action {
            name,
            name_span: name_token.span.clone(),
            kind,
            span: name_token.span.start..self.prev_end(),
        })
    }

    /// Whether the next token could be a property value.
    fn starts_value(&self) -> bool {
        match self.kind() {
            Some(TokenKind::Number | TokenKind::Hex | TokenKind::Str | TokenKind::Column) => true,
            Some(TokenKind::Ident) => {
                let text = self.token_text();
                catalog::action(text).is_none() && !catalog::is_reserved(text)
            }
            _ => false,
        }
    }

    /// The space-separated values after `property:`.
    fn values(&mut self, property: &str) -> Vec<Expr> {
        let mut values = Vec::new();
        loop {
            if self.eof() {
                self.note(Expect::Value { property: property.into(), index: values.len() });
            }
            let ends = match self.kind() {
                None | Some(TokenKind::Comma | TokenKind::Semi | TokenKind::RParen) => true,
                Some(TokenKind::Ident) => self.is_kw("else") || self.is_kw("elif"),
                _ => false,
            };
            if ends {
                if values.is_empty() {
                    self.error(format!("expected a value after `{property}:`"));
                }
                return values;
            }
            values.push(self.atom());
            if self.rule_failed {
                return values;
            }
        }
    }

    /// One value of a property or a formatting call: a literal, a name, a
    /// column or a call.
    fn atom(&mut self) -> Expr {
        let start = self.here();
        match self.kind() {
            Some(TokenKind::Minus) if self.kind_at(1) == Some(TokenKind::Number) => {
                self.bump();
                let number = self.primary();
                Expr { kind: ExprKind::Neg(Box::new(number)), span: start..self.prev_end() }
            }
            Some(TokenKind::LParen) | Some(TokenKind::Minus) => self.primary(),
            Some(kind) if is_operand(kind) => self.primary(),
            // What fits here was noted by the caller.
            None => {
                self.error("expected a value");
                Expr { kind: ExprKind::Error, span: start..start }
            }
            _ => {
                self.error(format!("expected a value, found {}", self.found()));
                Expr { kind: ExprKind::Error, span: start..start }
            }
        }
    }

    /// The arguments after `name(`, through the `)`.
    fn args(&mut self, name: &str, action: bool) -> Vec<Arg> {
        let mut args = Vec::new();
        let open = self.prev_end().saturating_sub(1);
        loop {
            // A formatting call's values are single words, except where it
            // takes an expression, like `display(c * 10)`.
            let expression = !action || catalog::takes_expression(name, args.len());
            if self.eof() {
                self.note(Expect::Arg { call: name.into(), index: args.len(), action });
                // Formatting calls take values completion lists itself, and
                // whole-column functions take a column.
                self.frozen |= (action && !expression)
                    || (args.is_empty()
                        && catalog::function(name).is_some_and(|f| f.kind != catalog::FunctionKind::Scalar));
            }
            if args.is_empty() && self.eat(TokenKind::RParen) {
                return args;
            }
            if self.eof() {
                self.error_at(open..open + 1, format!("`{name}(` is missing its `)`"));
                return args;
            }
            let start = self.here();
            let named = action
                && self.kind() == Some(TokenKind::Ident)
                && self.kind_at(1) == Some(TokenKind::Eq)
                && self.tokens[self.pos + 1].span.len() == 1;
            let arg = if named {
                let name_token = self.bump();
                self.bump();
                let arg_name = name_token.text(self.text).to_ascii_lowercase();
                if self.eof() {
                    self.note(Expect::NamedValue { call: name.into(), name: arg_name.clone() });
                    self.frozen = true;
                }
                let value = self.expr_or_atom(true);
                Arg { name: Some((arg_name, name_token.span)), value, span: start..self.prev_end() }
            } else {
                let value = self.expr_or_atom(expression);
                Arg { name: None, value, span: start..self.prev_end() }
            };
            args.push(arg);
            if self.rule_failed {
                return args;
            }
            if self.eat(TokenKind::Comma) {
                continue;
            }
            if self.eat(TokenKind::RParen) {
                return args;
            }
            if self.eof() {
                self.error_at(open..open + 1, format!("`{name}(` is missing its `)`"));
            } else {
                self.error(format!("expected `,` or `)` in `{name}(…)`, found {}", self.found()));
            }
            return args;
        }
    }

    fn expr_or_atom(&mut self, expr: bool) -> Expr {
        if expr { self.expr() } else { self.atom() }
    }

    fn expr(&mut self) -> Expr {
        let mut left = self.and_expr();
        loop {
            if !self.eat_kw("or") && !self.eat_symbol_word("||", "or") {
                return left;
            }
            let right = self.and_expr();
            left = binary(BinOp::Or, left, right);
        }
    }

    fn and_expr(&mut self) -> Expr {
        let mut left = self.not_expr();
        loop {
            if !self.eat_kw("and") && !self.eat_symbol_word("&&", "and") {
                return left;
            }
            let right = self.not_expr();
            left = binary(BinOp::And, left, right);
        }
    }

    /// `&&` and `||`, which read as `and` and `or` after a gentle error.
    fn eat_symbol_word(&mut self, symbol: &str, word: &str) -> bool {
        let first = symbol.as_bytes()[0] as char;
        if self.kind() != Some(TokenKind::Unknown) || !self.token_text().starts_with(first) {
            return false;
        }
        let start = self.here();
        self.bump();
        let mut end = self.prev_end();
        if self.kind() == Some(TokenKind::Unknown) && self.token_text().starts_with(first) {
            end = self.bump().span.end;
        }
        self.error_at(start..end, format!("use `{word}` instead of `{symbol}`"));
        true
    }

    fn not_expr(&mut self) -> Expr {
        let start = self.here();
        if self.eat_kw("not") {
            let inner = self.not_expr();
            return Expr { kind: ExprKind::Not(Box::new(inner)), span: start..self.prev_end() };
        }
        if self.kind() == Some(TokenKind::Unknown) && self.token_text() == "!" {
            self.error("use `not` instead of `!`");
            self.bump();
            let inner = self.not_expr();
            return Expr { kind: ExprKind::Not(Box::new(inner)), span: start..self.prev_end() };
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Expr {
        let start = self.here();
        let left = self.additive();
        if self.eof() {
            self.note(Expect::Operator);
            return left;
        }
        let op = match self.kind() {
            Some(TokenKind::Eq) => Some(BinOp::Eq),
            Some(TokenKind::Ne) => Some(BinOp::Ne),
            Some(TokenKind::Lt) => Some(BinOp::Lt),
            Some(TokenKind::Le) => Some(BinOp::Le),
            Some(TokenKind::Gt) => Some(BinOp::Gt),
            Some(TokenKind::Ge) => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let right = self.additive();
            return binary(op, left, right);
        }
        if self.eat_kw("is") {
            let negated = self.eat_kw("not");
            if !self.eat_kw("null") {
                self.error(format!("expected `null` after `is{}`", if negated { " not" } else { "" }));
            }
            return Expr { kind: ExprKind::IsNull { expr: Box::new(left), negated }, span: start..self.prev_end() };
        }
        let negated = self.is_kw("not")
            && self.tokens.get(self.pos + 1).is_some_and(|t| {
                let word = t.text(self.text).to_ascii_lowercase();
                ["between", "in", "like", "contains", "startswith", "endswith"].contains(&word.as_str())
            });
        if negated {
            self.bump();
        }
        let test = if self.eat_kw("between") {
            let low = self.additive();
            if !self.eat_kw("and") {
                self.error(format!("expected `and` in `between … and …`, found {}", self.found()));
            }
            let high = self.additive();
            ExprKind::Between { expr: Box::new(left), low: Box::new(low), high: Box::new(high), negated }
        } else if self.eat_kw("in") {
            let mut list = Vec::new();
            if !self.eat(TokenKind::LParen) {
                self.error("expected `(` and a list of values after `in`");
            } else {
                loop {
                    list.push(self.expr());
                    if self.rule_failed || !self.eat(TokenKind::Comma) {
                        break;
                    }
                }
                if !self.rule_failed && !self.eat(TokenKind::RParen) {
                    self.error(format!("expected `,` or `)` in the `in` list, found {}", self.found()));
                }
            }
            ExprKind::In { expr: Box::new(left), list, negated }
        } else {
            let op = if self.eat_kw("like") {
                BinOp::Like
            } else if self.eat_kw("contains") {
                BinOp::Contains
            } else if self.eat_kw("startswith") {
                BinOp::StartsWith
            } else if self.eat_kw("endswith") {
                BinOp::EndsWith
            } else {
                return left;
            };
            let right = self.additive();
            let test = binary(op, left, right);
            if !negated {
                return test;
            }
            ExprKind::Not(Box::new(test))
        };
        Expr { kind: test, span: start..self.prev_end() }
    }

    fn additive(&mut self) -> Expr {
        let mut left = self.multiplicative();
        loop {
            let op = match self.kind() {
                Some(TokenKind::Plus) => BinOp::Add,
                Some(TokenKind::Minus) => BinOp::Sub,
                _ => return left,
            };
            self.bump();
            let right = self.multiplicative();
            left = binary(op, left, right);
        }
    }

    fn multiplicative(&mut self) -> Expr {
        let mut left = self.unary();
        loop {
            let op = match self.kind() {
                Some(TokenKind::Star) => BinOp::Mul,
                Some(TokenKind::Slash) => BinOp::Div,
                Some(TokenKind::Percent) => BinOp::Rem,
                _ => return left,
            };
            self.bump();
            let right = self.unary();
            left = binary(op, left, right);
        }
    }

    fn unary(&mut self) -> Expr {
        let start = self.here();
        if self.kind() == Some(TokenKind::Minus) {
            self.bump();
            let inner = self.unary();
            return Expr { kind: ExprKind::Neg(Box::new(inner)), span: start..self.prev_end() };
        }
        self.primary()
    }

    /// A value and the list indexes and struct fields after it: `c[0].name`.
    fn primary(&mut self) -> Expr {
        let start = self.here();
        let mut expr = self.operand();
        if expr.kind == ExprKind::Error {
            return expr;
        }
        loop {
            match self.kind() {
                Some(TokenKind::LBracket) => {
                    self.bump();
                    let index = self.expr();
                    if !self.eat(TokenKind::RBracket) {
                        self.error(format!("expected `]`, found {}", self.found()));
                    }
                    let kind = ExprKind::Index { expr: Box::new(expr), index: Box::new(index) };
                    expr = Expr { kind, span: start..self.prev_end() };
                }
                Some(TokenKind::Dot) => {
                    self.bump();
                    let Some(token) = self.peek().cloned().filter(|t| matches!(t.kind, TokenKind::Ident | TokenKind::Column))
                    else {
                        if let Some(path) = FieldPath::of(&expr) {
                            self.note(Expect::Field(path));
                        }
                        self.error(format!("expected a field name after `.`, found {}", self.found()));
                        return Expr { kind: ExprKind::Error, span: start..self.prev_end() };
                    };
                    self.bump();
                    let text = token.text(self.text);
                    let name = if token.kind == TokenKind::Column { unquote(text) } else { text.to_string() };
                    let kind = ExprKind::Field { expr: Box::new(expr), name, name_span: token.span.clone() };
                    expr = Expr { kind, span: start..self.prev_end() };
                }
                _ => return expr,
            }
        }
    }

    fn operand(&mut self) -> Expr {
        let start = self.here();
        let error = |end: usize| Expr { kind: ExprKind::Error, span: start..end };
        let Some(token) = self.peek().cloned() else {
            self.note(Expect::Expr);
            self.error("expected a value, like `c`, a \"column\", a number or a function");
            return error(start);
        };
        let text = token.text(self.text);
        let kind = match token.kind {
            TokenKind::Number => {
                self.bump();
                number(text)
            }
            TokenKind::Str | TokenKind::Column => {
                self.bump();
                if !token.terminated {
                    let quote = &text[..1];
                    self.error_at(token.span.clone(), format!("this {quote} has no closing {quote}"));
                }
                if token.kind == TokenKind::Str {
                    ExprKind::Str(unquote(text))
                } else {
                    ExprKind::Column(unquote(text))
                }
            }
            TokenKind::Hex => {
                self.bump();
                ExprKind::Hex(text.to_string())
            }
            TokenKind::LParen => {
                self.bump();
                let inner = self.expr();
                if !self.eat(TokenKind::RParen) {
                    self.error(format!("expected `)`, found {}", self.found()));
                }
                return Expr { span: start..self.prev_end(), ..inner };
            }
            TokenKind::Ident => {
                let lower = text.to_ascii_lowercase();
                match lower.as_str() {
                    "c" => {
                        self.bump();
                        ExprKind::Cell
                    }
                    "true" | "false" => {
                        self.bump();
                        ExprKind::Bool(lower == "true")
                    }
                    "null" => {
                        self.bump();
                        ExprKind::Null
                    }
                    _ if catalog::is_reserved(&lower) => {
                        self.error(format!("expected a value before {}", self.found()));
                        return error(start);
                    }
                    _ if self.kind_at(1) == Some(TokenKind::LParen) => {
                        self.bump();
                        self.bump();
                        let action = catalog::function(&lower).is_none()
                            && catalog::action(&lower).is_some_and(|a| a.form == Form::Call);
                        let args = self.args(&lower, action);
                        ExprKind::Call { name: lower, name_span: token.span.clone(), args }
                    }
                    _ => {
                        self.bump();
                        ExprKind::Name(lower)
                    }
                }
            }
            TokenKind::Unknown => {
                self.error(format!("unexpected {}", self.found()));
                return error(start);
            }
            _ => {
                self.error(format!("expected a value, found {}", self.found()));
                return error(start);
            }
        };
        Expr { kind, span: start..self.prev_end() }
    }
}

fn is_operand(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Number | TokenKind::Str | TokenKind::Column | TokenKind::Hex | TokenKind::Ident
    )
}

fn binary(op: BinOp, left: Expr, right: Expr) -> Expr {
    let span = left.span.start..right.span.end.max(left.span.end);
    Expr { kind: ExprKind::Binary(op, Box::new(left), Box::new(right)), span }
}

fn number(text: &str) -> ExprKind {
    let lower = text.to_ascii_lowercase();
    if let Some(digits) = lower.strip_suffix("px") {
        ExprKind::Length(digits.parse().unwrap_or(0.0))
    } else if let Some(digits) = lower.strip_suffix('%') {
        ExprKind::Percent(digits.parse().unwrap_or(0.0))
    } else {
        ExprKind::Number(lower.parse().unwrap_or(0.0))
    }
}

fn symbol(kind: TokenKind) -> Option<&'static str> {
    Some(match kind {
        TokenKind::LParen => "(",
        TokenKind::RParen => ")",
        TokenKind::Comma => ",",
        TokenKind::Semi => ";",
        TokenKind::RBracket => "]",
        TokenKind::Colon => ":",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(text: &str) -> Vec<(String, String)> {
        parse(text)
            .diagnostics
            .into_iter()
            .map(|d| (text[d.span.clone()].to_string(), d.message))
            .collect()
    }

    #[test]
    fn the_examples_parse() {
        let parsed = parse("if c > \"column1\" then bg:red , border:1px, border_weight:1px, text:green");
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let rule = &parsed.rules.rules[0];
        assert_eq!(rule.branches[0].actions.len(), 4);
        let Some(Expr { kind: ExprKind::Binary(BinOp::Gt, left, right), .. }) = &rule.branches[0].condition else {
            panic!("{rule:?}");
        };
        assert_eq!(left.kind, ExprKind::Cell);
        assert_eq!(right.kind, ExprKind::Column("column1".into()));
    }

    #[test]
    fn rules_elifs_and_calls_parse() {
        let text = "if c > 0 then text: green elif c < 0 then text: red else text: gray; \
                    scale(red, white, green, mid=0); \
                    if c not between 1 and avg(c) * 2 or c is not null then bar(#638ec6, fill=gradient)";
        let parsed = parse(text);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(parsed.rules.rules.len(), 3);
        assert_eq!(parsed.rules.rules[0].branches.len(), 2);
        assert!(parsed.rules.rules[0].otherwise.is_some());
        let ActionKind::Call(args) = &parsed.rules.rules[1].branches[0].actions[0].kind else { panic!() };
        assert_eq!(args[3].name.as_ref().map(|n| n.0.as_str()), Some("mid"));
    }

    #[test]
    fn mistakes_get_specific_messages() {
        assert_eq!(errors("if c > 5 bg: red"), [("bg".into(), "expected `then` before `bg`".into())]);
        assert_eq!(
            errors("if c > 5 then bold\nif c < 0 then italic"),
            [("if".into(), "expected `;` before the next rule: rules are separated by `;`".into())]
        );
        assert_eq!(errors("if c > 1 && c < 5 then bold"), [("&&".into(), "use `and` instead of `&&`".into())]);
        assert_eq!(errors("bg: red bold"), []);
        assert_eq!(errors("bg red"), [("red".into(), "expected `:` after `bg`".into())]);
        assert_eq!(
            errors("if c = 'open then bold"),
            [("'open then bold".into(), "this ' has no closing '".into())]
        );
        assert_eq!(errors("bar(green"), [("(".into(), "`bar(` is missing its `)`".into())]);
        assert_eq!(errors("if c > then bold"), [("then".into(), "expected a value before `then`".into())]);
    }

    #[test]
    fn one_error_per_rule_and_later_rules_still_parse() {
        let parsed = parse("if c >> 1 then bold; if c < 0 then italic");
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.rules.rules.len(), 2);
        assert_eq!(parsed.rules.rules[1].branches[0].actions[0].name, "italic");
    }

    fn expect(text: &str) -> Vec<Expect> {
        expected(text, tokenize(text))
    }

    #[test]
    fn the_end_of_the_text_knows_what_fits() {
        assert_eq!(expect(""), [Expect::Keyword("if"), Expect::Action]);
        assert_eq!(expect("if "), [Expect::Keyword("not"), Expect::Expr]);
        let after_value = expect("if c ");
        assert!(after_value.contains(&Expect::Operator));
        assert!(after_value.contains(&Expect::Keyword("then")));
        assert!(after_value.contains(&Expect::Keyword("and")));
        assert_eq!(expect("if c > 1 then "), [Expect::Action]);
        assert_eq!(expect("bg: "), [Expect::Value { property: "bg".into(), index: 0 }]);
        let after_action = expect("if c > 1 then bold ");
        assert!(after_action.contains(&Expect::Symbol(",")));
        assert!(after_action.contains(&Expect::Keyword("else")));
        assert!(after_action.contains(&Expect::Symbol(";")));
        assert_eq!(expect("bar(green, "), [Expect::Arg { call: "bar".into(), index: 1, action: true }]);
        assert_eq!(expect("bar(green, fill="), [Expect::NamedValue { call: "bar".into(), name: "fill".into() }]);
        assert!(expect("if rank(").contains(&Expect::Arg { call: "rank".into(), index: 0, action: false }));
    }
}
