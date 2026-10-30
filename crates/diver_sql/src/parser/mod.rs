//! The parser: recursive descent over the non-trivia tokens, emitting events.
//!
//! Words reach the parser as `IDENT`; `at_kw` / `bump_kw` read and remap them,
//! so a word is a keyword only where the grammar asks for one.
//!
//! For completion the token list carries a hole (`CURSOR`). Every check the
//! grammar makes at the hole fails and is recorded in `expected`, so the
//! parser collects everything it would accept there.

pub(crate) mod builder;
pub(crate) mod event;
mod grammar;

use std::cell::Cell;

use rowan::GreenNode;

use crate::complete::Expected;
use crate::lexer::Token;
use crate::{Dialect, Entry, Kw, ParseOptions, SyntaxError, SyntaxKind};
use event::Event;

/// Checks without consuming a token before the parser counts as stuck.
const FUEL: u32 = 16384;

/// How deep expressions, FROM items and types may nest before the rest of
/// the statement is skipped, so deep input can't overflow the stack.
const MAX_NESTING: u32 = 500;

pub(crate) struct Parser<'a> {
    kinds: Vec<SyntaxKind>,
    texts: Vec<&'a str>,
    /// The keyword each word spells, looked up once.
    kws: Vec<Option<Kw>>,
    pos: usize,
    /// One past the last token the current statement may use (its `;`).
    limit: usize,
    events: Vec<Event>,
    steps: Cell<u32>,
    stuck: Cell<bool>,
    hole: Option<usize>,
    string_hole: Option<usize>,
    expected: Vec<Expected>,
    nesting: u32,
    #[allow(dead_code)] // the Polars grammar gates read it
    pub(crate) dialect: Dialect,
}

pub(crate) struct ParseOutput {
    pub green: GreenNode,
    pub errors: Vec<SyntaxError>,
    pub expected: Vec<Expected>,
}

/// Parses `tokens` (all of them, trivia included) of `text`. `hole` is the index
/// of the `CURSOR` token and `string_hole` the index of a string the cursor is
/// in, both counted over all tokens.
pub(crate) fn parse_tokens(
    text: &str,
    tokens: &[Token],
    options: ParseOptions,
    hole: Option<usize>,
    string_hole: Option<usize>,
) -> ParseOutput {
    let mut kinds = Vec::with_capacity(tokens.len());
    let mut texts = Vec::with_capacity(tokens.len());
    let mut kws = Vec::with_capacity(tokens.len());
    let (mut parser_hole, mut parser_string_hole) = (None, None);
    let mut offset = 0usize;
    for (i, t) in tokens.iter().enumerate() {
        let len = usize::from(t.len);
        if !t.kind.is_trivia() {
            if Some(i) == hole {
                parser_hole = Some(kinds.len());
            }
            if Some(i) == string_hole {
                parser_string_hole = Some(kinds.len());
            }
            kinds.push(t.kind);
            let word = &text[offset..offset + len];
            kws.push(if t.kind == SyntaxKind::IDENT {
                Kw::lookup(word)
            } else {
                None
            });
            texts.push(word);
        }
        offset += len;
    }
    let limit = kinds.len();
    let mut p = Parser {
        kinds,
        texts,
        kws,
        pos: 0,
        limit,
        events: Vec::new(),
        steps: Cell::new(0),
        stuck: Cell::new(false),
        hole: parser_hole,
        string_hole: parser_string_hole,
        expected: Vec::new(),
        nesting: 0,
        dialect: options.dialect,
    };
    match options.entry {
        Entry::Script => grammar::source_file(&mut p),
        Entry::Expression => grammar::expression_root(&mut p),
    }
    let expected = std::mem::take(&mut p.expected);
    let (green, errors) = builder::build(text, tokens, p.events);
    ParseOutput {
        green,
        errors,
        expected,
    }
}

impl<'a> Parser<'a> {
    fn fuel(&self) -> bool {
        let steps = self.steps.get() + 1;
        self.steps.set(steps);
        if steps > FUEL {
            self.stuck.set(true);
        }
        !self.stuck.get()
    }

    pub(crate) fn nth(&self, n: usize) -> SyntaxKind {
        if !self.fuel() {
            return SyntaxKind::EOF;
        }
        let i = self.pos + n;
        if i < self.limit {
            self.kinds[i]
        } else {
            SyntaxKind::EOF
        }
    }

    /// The n-th token without spending fuel, for bounded lookahead scans.
    pub(crate) fn raw_nth(&self, n: usize) -> SyntaxKind {
        let i = self.pos + n;
        if i < self.limit {
            self.kinds[i]
        } else {
            SyntaxKind::EOF
        }
    }

    /// The keyword the n-th token spells, without spending fuel.
    pub(crate) fn raw_nth_kw(&self, n: usize) -> Option<Kw> {
        let i = self.pos + n;
        if i < self.limit { self.kws[i] } else { None }
    }

    pub(crate) fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    pub(crate) fn nth_text(&self, n: usize) -> &'a str {
        let i = self.pos + n;
        if i < self.limit { self.texts[i] } else { "" }
    }

    /// The keyword the n-th token spells, if it is a plain word.
    pub(crate) fn nth_kw(&self, n: usize) -> Option<Kw> {
        if self.fuel() {
            self.raw_nth_kw(n)
        } else {
            None
        }
    }

    pub(crate) fn at_eof(&self) -> bool {
        self.current() == SyntaxKind::EOF
    }

    /// Whether the current token is the completion hole.
    pub(crate) fn at_hole(&self) -> bool {
        self.hole == Some(self.pos) && self.pos < self.limit && !self.stuck.get()
    }

    /// Whether the current token is the string the cursor is in.
    pub(crate) fn at_string_hole(&self) -> bool {
        self.string_hole == Some(self.pos)
    }

    /// Records what the string the cursor is in names (a file, an extension).
    pub(crate) fn record_string(&mut self, expected: Expected) {
        if self.at_string_hole() && !self.expected.contains(&expected) {
            self.expected.push(expected);
        }
    }

    /// The text of the token before the current one.
    pub(crate) fn prev_text(&self) -> &'a str {
        if self.pos == 0 {
            ""
        } else {
            self.texts[self.pos - 1]
        }
    }

    pub(crate) fn record(&mut self, expected: Expected) {
        if self.at_hole() && !self.expected.contains(&expected) {
            self.expected.push(expected);
        }
    }

    pub(crate) fn at(&mut self, kind: SyntaxKind) -> bool {
        if kind.is_punct() {
            self.record(Expected::Punct(kind));
        }
        self.current() == kind
    }

    pub(crate) fn at_kw(&mut self, kw: Kw) -> bool {
        self.record(Expected::Keyword(kw));
        self.nth_kw(0) == Some(kw)
    }

    /// `at_kw` for any of `kws`.
    pub(crate) fn at_any_kw(&mut self, kws: &[Kw]) -> bool {
        let mut hit = false;
        for &kw in kws {
            hit |= self.at_kw(kw);
        }
        hit
    }

    /// Checks for a keyword without recording it as an expectation.
    pub(crate) fn peek_kw(&self, kw: Kw) -> bool {
        self.nth_kw(0) == Some(kw)
    }

    pub(crate) fn nth_at_kw(&self, n: usize, kw: Kw) -> bool {
        self.nth_kw(n) == Some(kw)
    }

    /// Checks for the first word of a phrase and records the whole phrase.
    pub(crate) fn at_phrase(&mut self, phrase: &'static [Kw]) -> bool {
        self.record(Expected::Phrase(phrase));
        self.peek_kw(phrase[0])
    }

    /// Takes the current token as it is.
    pub(crate) fn bump(&mut self) {
        let kind = self.current();
        if kind == SyntaxKind::EOF {
            return;
        }
        self.bump_as(kind);
    }

    pub(crate) fn bump_as(&mut self, kind: SyntaxKind) {
        if self.pos >= self.limit {
            return;
        }
        self.pos += 1;
        self.steps.set(0);
        self.events.push(Event::Token { kind });
    }

    pub(crate) fn bump_kw(&mut self, kw: Kw) {
        self.bump_as(kw.into());
    }

    /// Takes the current word as a keyword, whichever it is.
    pub(crate) fn bump_any_kw(&mut self) {
        match self.nth_kw(0) {
            Some(kw) => self.bump_kw(kw),
            None => self.bump(),
        }
    }

    pub(crate) fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(crate) fn eat_kw(&mut self, kw: Kw) -> bool {
        if self.at_kw(kw) {
            self.bump_kw(kw);
            true
        } else {
            false
        }
    }

    /// Eats a list's comma; false when there is none, or when it is a
    /// trailing comma (DuckDB allows them in most lists).
    pub(crate) fn eat_list_comma(&mut self) -> bool {
        if !self.eat(SyntaxKind::COMMA) {
            return false;
        }
        self.at_hole() || !(self.at_recovery(&[]) || self.current() == SyntaxKind::SEMICOLON)
    }

    pub(crate) fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        self.error(format!("expected {}", describe(kind)));
        false
    }

    pub(crate) fn expect_kw(&mut self, kw: Kw) -> bool {
        if self.eat_kw(kw) {
            return true;
        }
        self.error(format!("expected {}", kw.text().to_ascii_uppercase()));
        false
    }

    pub(crate) fn error(&mut self, msg: impl Into<String>) {
        self.events.push(Event::Error { msg: msg.into() });
    }

    /// Reports an error and, unless the current token is one a caller can
    /// continue from, wraps tokens in an `ERROR` node until it reaches one.
    pub(crate) fn err_recover(&mut self, msg: impl Into<String>, stop: &[Kw]) {
        self.error(msg);
        if self.at_recovery(stop) {
            return;
        }
        let m = self.start();
        self.skip_until(stop);
        m.complete(self, SyntaxKind::ERROR);
    }

    /// Clause keywords and closing brackets: places an enclosing rule can go on from.
    pub(crate) fn at_recovery(&self, stop: &[Kw]) -> bool {
        const CLAUSES: &[Kw] = &[
            Kw::From,
            Kw::Where,
            Kw::Group,
            Kw::Having,
            Kw::Window,
            Kw::Qualify,
            Kw::Order,
            Kw::Limit,
            Kw::Offset,
            Kw::Union,
            Kw::Except,
            Kw::Intersect,
            Kw::Returning,
        ];
        match self.current() {
            SyntaxKind::EOF | SyntaxKind::R_PAREN | SyntaxKind::R_BRACK | SyntaxKind::R_CURLY => {
                true
            }
            SyntaxKind::IDENT => match self.nth_kw(0) {
                Some(kw) => CLAUSES.contains(&kw) || stop.contains(&kw),
                None => false,
            },
            _ => false,
        }
    }

    pub(crate) fn skip_until(&mut self, stop: &[Kw]) {
        while !self.at_recovery(stop) {
            self.bump_one_balanced();
        }
    }

    /// Bumps one token, or a whole bracketed group when at an opening bracket.
    pub(crate) fn bump_one_balanced(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.current() {
                SyntaxKind::EOF => return,
                SyntaxKind::L_PAREN | SyntaxKind::L_BRACK | SyntaxKind::L_CURLY => depth += 1,
                SyntaxKind::R_PAREN | SyntaxKind::R_BRACK | SyntaxKind::R_CURLY => {
                    if depth == 0 {
                        self.bump();
                        return;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            self.bump();
            if depth == 0 {
                return;
            }
        }
    }

    pub(crate) fn start(&mut self) -> Marker {
        let pos = self.events.len() as u32;
        self.events.push(Event::Start {
            kind: SyntaxKind::ERROR,
            forward_parent: None,
        });
        Marker { pos }
    }

    pub(crate) fn is_stuck(&self) -> bool {
        self.stuck.get()
    }

    /// Ends the statement being parsed at token `limit` (its `;`), or lifts the end.
    pub(crate) fn set_limit(&mut self, limit: Option<usize>) {
        self.limit = limit.unwrap_or(self.kinds.len());
    }

    /// The index of the next `;` at or after the current token.
    pub(crate) fn next_semicolon(&self) -> Option<usize> {
        (self.pos..self.kinds.len()).find(|&i| self.kinds[i] == SyntaxKind::SEMICOLON)
    }

    /// Bumps everything up to the limit into an `ERROR` node, fuel or not.
    pub(crate) fn bump_rest_as_error(&mut self) {
        self.stuck.set(false);
        self.steps.set(0);
        if self.pos >= self.limit {
            return;
        }
        let m = self.start();
        while self.pos < self.limit {
            let kind = self.kinds[self.pos];
            self.bump_as(kind);
        }
        m.complete(self, SyntaxKind::ERROR);
    }

    /// Runs a rule one level deeper; past `MAX_NESTING` it skips the rest of
    /// the statement instead.
    pub(crate) fn nested<T>(&mut self, rule: impl FnOnce(&mut Self) -> Option<T>) -> Option<T> {
        if self.nesting >= MAX_NESTING {
            self.error("nested too deeply");
            self.bump_rest_as_error();
            return None;
        }
        self.nesting += 1;
        let out = rule(self);
        self.nesting -= 1;
        out
    }

    pub(crate) fn reset_fuel(&mut self) {
        self.stuck.set(false);
        self.steps.set(0);
    }
}

fn describe(kind: SyntaxKind) -> String {
    let text = match kind {
        SyntaxKind::L_PAREN => "'('",
        SyntaxKind::R_PAREN => "')'",
        SyntaxKind::L_BRACK => "'['",
        SyntaxKind::R_BRACK => "']'",
        SyntaxKind::L_CURLY => "'{'",
        SyntaxKind::R_CURLY => "'}'",
        SyntaxKind::COMMA => "','",
        SyntaxKind::COLON => "':'",
        SyntaxKind::EQ => "'='",
        SyntaxKind::DOT => "'.'",
        SyntaxKind::IDENT => "a name",
        _ => return format!("{kind:?}").to_lowercase(),
    };
    text.to_string()
}

pub(crate) struct Marker {
    pos: u32,
}

impl Marker {
    pub(crate) fn complete(self, p: &mut Parser, kind: SyntaxKind) -> CompletedMarker {
        match &mut p.events[self.pos as usize] {
            Event::Start { kind: slot, .. } => *slot = kind,
            _ => unreachable!(),
        }
        p.events.push(Event::Finish);
        CompletedMarker { pos: self.pos }
    }

    /// Drops the node. The event stays as a tombstone, since a node that
    /// was `precede`d may point at it.
    pub(crate) fn abandon(self, p: &mut Parser) {
        p.events[self.pos as usize] = Event::Tombstone;
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CompletedMarker {
    pos: u32,
}

impl CompletedMarker {
    /// Starts a node that will contain this one.
    pub(crate) fn precede(self, p: &mut Parser) -> Marker {
        let m = p.start();
        match &mut p.events[self.pos as usize] {
            Event::Start { forward_parent, .. } => *forward_parent = Some(m.pos - self.pos),
            _ => unreachable!(),
        }
        m
    }
}
