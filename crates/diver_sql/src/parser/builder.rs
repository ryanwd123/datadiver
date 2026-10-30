//! Turns parser events and the lexer's tokens into a rowan tree.

use rowan::{GreenNode, GreenNodeBuilder, TextRange, TextSize};

use super::event::Event;
use crate::lexer::Token;
use crate::{SyntaxError, SyntaxKind};

pub(crate) fn build(
    text: &str,
    tokens: &[Token],
    mut events: Vec<Event>,
) -> (GreenNode, Vec<SyntaxError>) {
    let mut b = Builder {
        text,
        tokens,
        idx: 0,
        offset: TextSize::new(0),
        inner: GreenNodeBuilder::new(),
        depth: 0,
    };
    let mut errors = Vec::new();
    let mut parents = Vec::new();
    for i in 0..events.len() {
        match std::mem::replace(&mut events[i], Event::Tombstone) {
            Event::Start {
                kind,
                forward_parent,
            } => {
                // Follow the forward_parent chain and open the outermost node first.
                parents.push(kind);
                let mut fp = forward_parent;
                let mut at = i;
                while let Some(delta) = fp {
                    at += delta as usize;
                    fp = match std::mem::replace(&mut events[at], Event::Tombstone) {
                        Event::Start {
                            kind,
                            forward_parent,
                        } => {
                            parents.push(kind);
                            forward_parent
                        }
                        // an abandoned `precede`
                        _ => None,
                    };
                }
                for kind in parents.drain(..).rev() {
                    b.start(kind);
                }
            }
            Event::Finish => b.finish(),
            Event::Token { kind } => b.token(kind),
            Event::Error { msg } => errors.push(SyntaxError {
                message: msg,
                range: b.next_range(),
            }),
            Event::Tombstone => {}
        }
    }
    (b.inner.finish(), errors)
}

struct Builder<'a> {
    text: &'a str,
    tokens: &'a [Token],
    idx: usize,
    offset: TextSize,
    inner: GreenNodeBuilder<'static>,
    depth: usize,
}

impl Builder<'_> {
    fn start(&mut self, kind: SyntaxKind) {
        // Trivia before a node goes to its parent, so nodes start at a real token.
        if self.depth > 0 {
            self.trivia();
        }
        self.inner.start_node(rowan::SyntaxKind(kind.0));
        self.depth += 1;
    }

    fn finish(&mut self) {
        self.depth -= 1;
        if self.depth == 0 {
            self.trivia();
            // Tokens the parser never looked at still belong in the tree.
            while self.idx < self.tokens.len() {
                let kind = self.tokens[self.idx].kind;
                self.raw(kind);
            }
        }
        self.inner.finish_node();
    }

    fn token(&mut self, kind: SyntaxKind) {
        self.trivia();
        if self.idx < self.tokens.len() {
            self.raw(kind);
        }
    }

    fn trivia(&mut self) {
        while self.idx < self.tokens.len() && self.tokens[self.idx].kind.is_trivia() {
            let kind = self.tokens[self.idx].kind;
            self.raw(kind);
        }
    }

    fn raw(&mut self, kind: SyntaxKind) {
        let len = self.tokens[self.idx].len;
        let range = TextRange::at(self.offset, len);
        self.inner
            .token(rowan::SyntaxKind(kind.0), &self.text[range]);
        self.offset += len;
        self.idx += 1;
    }

    /// The range of the next non-trivia token, or the end of the text.
    fn next_range(&self) -> TextRange {
        let mut offset = self.offset;
        for t in &self.tokens[self.idx..] {
            if !t.kind.is_trivia() {
                return TextRange::at(offset, t.len);
            }
            offset += t.len;
        }
        TextRange::empty(offset)
    }
}
