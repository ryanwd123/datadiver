//! An embeddable native Windows code editor component, modeled on Monaco's editor.
//!
//! [`Editor::create`] makes the editor a child window of any Win32 window. The
//! host positions it with [`Editor::set_bounds`], shows or hides it like a
//! panel, and listens to [`Event`]s. Several editors can live in one process;
//! they share the Direct2D/DirectWrite factories and compiled grammars.
//!
//! Hosts that draw everything into one window use [`Editor::create_hosted`]
//! instead, forward input with [`Editor::handle_message`] and draw the editor
//! with [`Editor::paint_into`].
//!
//! ```no_run
//! use code_editor::{Decoration, DecorationOptions, Editor, Event, Options, Pos, Severity};
//! # fn host(parent: windows::Win32::Foundation::HWND) -> windows::core::Result<()> {
//! let ed = Editor::create(parent, Options::default())?;
//! ed.set_bounds(0, 0, 800, 600);
//! ed.set_text("SELECT * FROM users WHERE id = 1;\n");
//! ed.set_language("sql");
//! ed.add_command("Ctrl+S", 1);
//! ed.on_event(|ed, e| match e {
//!     Event::Command(1) => println!("save {} bytes", ed.to_bytes().len()),
//!     Event::ContentChanged { changes, .. } => println!("{} edits", changes.len()),
//!     _ => {}
//! });
//! // Mark an error the host found (it moves with later edits).
//! ed.delta_decorations(&[], &[Decoration {
//!     start: Pos::new(0, 14),
//!     end: Pos::new(0, 19),
//!     options: DecorationOptions {
//!         squiggle: Some(Severity::Error),
//!         hover: Some("no such table".into()),
//!         ..Default::default()
//!     },
//! }]);
//! # Ok(())
//! # }
//! ```
//!
//! Languages are optional `lang-*` features (`lang-python`, `lang-sql`), each
//! registered only in [`lang::LANGS`]; a file whose language is not compiled
//! in opens as plain text.

pub mod commands;
pub mod decorations;
pub mod doc;
mod dragdrop;
mod editor;
pub mod history;
pub mod indent;
pub mod keymap;
pub mod lang;
pub mod movement;
mod platform;
mod render;
pub mod search;
pub mod selection;
pub mod syntax;
pub mod text;
pub mod theme;
pub mod view;
pub mod widgets;
mod window;

use std::path::PathBuf;

pub use keymap::{Action, Chord};
pub use text::{Encoding, Eol, Pos};
pub use theme::{Ink, Theme};
pub use decorations::{DecorationId, DecorationOptions, Severity, Stickiness};
pub use theme::Paint;
pub use widgets::suggest::{CompletionContext, CompletionItem, CompletionKind, CompletionProvider};
pub use window::{Decoration, Editor, TextEdit};

/// Editor options; [`Options::default`] matches Monaco's defaults where they
/// apply.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// The code font; `None` uses Cascadia Mono, Consolas or Courier New.
    pub font_family: Option<String>,
    /// Font size in DIPs before zoom.
    pub font_size: f32,
    pub tab_size: usize,
    pub insert_spaces: bool,
    /// Detect tabs or spaces and the tab size when text is loaded.
    pub detect_indentation: bool,
    pub line_numbers: bool,
    pub render_whitespace: bool,
    pub indent_guides: bool,
    pub highlight_current_line: bool,
    pub match_brackets: bool,
    /// Highlight other occurrences of the word at the caret.
    pub occurrences_highlight: bool,
    pub auto_closing: bool,
    pub folding: bool,
    pub word_wrap: bool,
    pub read_only: bool,
    pub context_menu: bool,
    /// Ctrl+wheel zooms.
    pub mouse_wheel_zoom: bool,
    pub cursor_blinking: bool,
    /// Open the suggest popup while typing words and on trigger characters.
    pub quick_suggestions: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            font_family: None,
            font_size: 14.0,
            tab_size: 4,
            insert_spaces: true,
            detect_indentation: true,
            line_numbers: true,
            render_whitespace: false,
            indent_guides: true,
            highlight_current_line: true,
            match_brackets: true,
            occurrences_highlight: true,
            auto_closing: true,
            folding: true,
            word_wrap: false,
            read_only: false,
            context_menu: true,
            mouse_wheel_zoom: true,
            cursor_blinking: true,
            quick_suggestions: true,
        }
    }
}

/// A selection in line/column positions; `head` is where the caret is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub anchor: Pos,
    pub head: Pos,
}

impl Range {
    pub fn caret(p: Pos) -> Self {
        Range { anchor: p, head: p }
    }

    pub fn start(&self) -> Pos {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> Pos {
        self.anchor.max(self.head)
    }
}

/// One replaced range in a content change. `start` and `end` are positions in
/// the text before the change; `text` replaced them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentChange {
    pub start: Pos,
    pub end: Pos,
    pub text: String,
}

/// Something the host may want to react to. Events are delivered after the
/// editor finishes handling a message, so handlers may call back into it.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The text changed. Changes are in descending order, each in positions of
    /// the text before this event, as in Monaco's `onDidChangeContent`.
    ContentChanged {
        version: u64,
        changes: Vec<ContentChange>,
    },
    SelectionChanged,
    Focus(bool),
    /// A host command bound with [`Editor::add_command`] was invoked.
    Command(u32),
    FilesDropped(Vec<PathBuf>),
    /// Loaded text had invalid bytes, which were replaced.
    Warning(String),
    /// The language changed (`None`: plain text).
    LanguageChanged(Option<&'static str>),
}

/// Writes `msg` with the time since process start when `WIN_MONACO_TRACE` is
/// set ("1" for OutputDebugString, or a file path).
pub fn trace(msg: &str) {
    if platform::tracing() {
        platform::trace(&format!("{:.1} ms: {msg}", platform::ms_since_process_start()));
    }
}
