//! What the parser emits: a flat list of events the tree builder turns into a tree.

use crate::SyntaxKind;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// Starts a node. `forward_parent` points (relatively) at a later `Start`
    /// that becomes this node's parent; it is how `Marker::precede` wraps a
    /// node that was already finished.
    Start {
        kind: SyntaxKind,
        forward_parent: Option<u32>,
    },
    Finish,
    /// The next non-trivia token, with the kind the parser gave it (a word the
    /// parser read as a keyword gets the keyword's kind).
    Token {
        kind: SyntaxKind,
    },
    /// An error at the next token.
    Error {
        msg: String,
    },
    /// An abandoned `Start`.
    Tombstone,
}
