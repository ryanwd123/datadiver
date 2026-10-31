//! Language features a host computes itself, such as an app's own SQL parser:
//! highlighting, problems, hovers, signature help, folding and smart select.
//! Every method has a default, so a service answers only what it knows; the
//! language's tree-sitter grammar, when there is one, covers the rest.

use std::ops::Range;

use crate::decorations::Severity;
use crate::theme::Ink;

/// A problem to underline. Its message shows when the mouse rests on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range<usize>,
    pub severity: Severity,
    pub message: String,
}

/// The call around the caret, shown above it while its arguments are typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignatureHelp {
    /// What the popup shows, like `sum(arg)`.
    pub label: String,
    /// The byte range of the argument being typed, in `label`.
    pub active: Option<Range<usize>>,
    /// Shown dimmed under the label.
    pub documentation: Option<String>,
}

/// Calls are synchronous and get the whole text; keep them fast.
pub trait LanguageService {
    /// Colors over the whole text, sorted and not overlapping; `None` leaves
    /// highlighting to the grammar. Asked once per change of the text.
    fn highlight(&self, _text: &str) -> Option<Vec<(Range<usize>, Ink)>> {
        None
    }

    /// Problems to underline, asked once typing pauses.
    fn diagnostics(&self, _text: &str) -> Vec<Diagnostic> {
        Vec::new()
    }

    /// Plain text for the mouse resting at `offset`. Lines are kept and long
    /// ones wrap.
    fn hover(&self, _text: &str, _offset: usize) -> Option<String> {
        None
    }

    /// The call around `offset`, asked after `(` or `,` is typed and while
    /// the popup is open.
    fn signature_help(&self, _text: &str, _offset: usize) -> Option<SignatureHelp> {
        None
    }

    /// Ranges that fold; `None` leaves folding to the grammar or indentation.
    fn folding_ranges(&self, _text: &str) -> Option<Vec<Range<usize>>> {
        None
    }

    /// Ranges around `offset`, innermost first, for Expand Selection; `None`
    /// leaves it to the grammar.
    fn selection_ranges(&self, _text: &str, _offset: usize) -> Option<Vec<Range<usize>>> {
        None
    }

    /// Where the name at `offset` is defined, for Go to Definition.
    fn definition(&self, _text: &str, _offset: usize) -> Option<Range<usize>> {
        None
    }

    /// Every use of what the name at `offset` refers to, its definition included.
    fn references(&self, _text: &str, _offset: usize) -> Vec<Range<usize>> {
        Vec::new()
    }

    /// The ranges Rename edits together for the name at `offset`, or why it
    /// can't be renamed.
    fn rename(&self, _text: &str, _offset: usize) -> Result<Vec<Range<usize>>, String> {
        Err("Nothing to rename here".into())
    }
}
