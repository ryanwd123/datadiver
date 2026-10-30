//! A headless window around a [`DataFrameView`] for the view's tests.

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use duckdb::types::Value;

use super::{DataFrameView, FileEntry};
use crate::domain::documents::DocumentDefinition;
use crate::domain::frame::FrameSnapshot;
use crate::domain::selection::Cell;
use crate::infrastructure::duckdb_store::DuckDbStore;
use crate::infrastructure::json_store::{StorePaths, ViewRepository};
use crate::ui::hit::Hit;
use crate::ui::platform::headless::TestApp;
use crate::ui::platform::system::fake;
use crate::ui::platform::{Cx, Point, Rect, WindowId};
use crate::ui::widgets::WidgetId;

pub(super) const WIDTH: f32 = 900.0;
pub(super) const HEIGHT: f32 = 600.0;

pub(super) fn repository(home: &Path) -> ViewRepository {
    ViewRepository::new(StorePaths::for_home(home))
}

/// A frame of `columns` (name, DuckDB type) holding `rows`.
pub(super) fn frame(columns: &[(&str, &str)], rows: Vec<Vec<Value>>) -> FrameSnapshot {
    let columns = columns
        .iter()
        .map(|(name, kind)| ((*name).to_owned(), (*kind).to_owned()))
        .collect::<Vec<_>>();
    FrameSnapshot::new(DuckDbStore::from_rows(&columns, rows).unwrap(), 1)
}

/// Two regions with their sales.
pub(super) fn sales_frame() -> FrameSnapshot {
    frame(
        &[("region", "VARCHAR"), ("sales", "BIGINT")],
        vec![
            vec![Value::Text("North".into()), Value::BigInt(10)],
            vec![Value::Text("South".into()), Value::BigInt(20)],
        ],
    )
}

pub(super) fn empty_frame() -> FrameSnapshot {
    FrameSnapshot::new(DuckDbStore::empty().unwrap(), 1)
}

/// The last URL the view opened in the browser.
pub(super) fn opened_url() -> Option<String> {
    fake::with(|system| system.opened_urls.last().cloned())
}

pub(super) fn clipboard_text() -> Option<String> {
    crate::ui::platform::system::read_clipboard_text()
}

pub(super) fn set_clipboard(text: &str) {
    crate::ui::platform::system::write_clipboard(text);
}

/// A window of the test app showing a [`DataFrameView`].
#[derive(Clone)]
pub(super) struct Viewer {
    pub app: Rc<TestApp<DataFrameView>>,
    pub id: WindowId,
}

impl Viewer {
    /// Opens a 900 × 600 window on `frame` in a new app.
    pub fn open(
        frame: FrameSnapshot,
        source_path: Option<String>,
        document: Option<DocumentDefinition>,
        repository: ViewRepository,
    ) -> Self {
        let app = Rc::new(TestApp::new());
        let viewer = Self::open_in(&app, frame, source_path, document, repository);
        viewer.render();
        viewer
    }

    /// Opens another window in `app`.
    pub fn open_in(
        app: &Rc<TestApp<DataFrameView>>,
        frame: FrameSnapshot,
        source_path: Option<String>,
        document: Option<DocumentDefinition>,
        repository: ViewRepository,
    ) -> Self {
        let id = app.open(WIDTH, HEIGHT, move |cx| {
            let mut view = DataFrameView::new(frame, source_path, String::new(), document, repository, cx);
            view.focus_grid(cx);
            view
        });
        let viewer = Self { app: app.clone(), id };
        viewer.run_until_parked();
        viewer
    }

    /// An empty viewer that then opens `path` like the file picker would,
    /// with settings in the file's folder.
    pub fn file(path: &Path) -> Self {
        let viewer = Self::open(empty_frame(), None, None, repository(path.parent().unwrap()));
        viewer.update(|view, cx| {
            view.navigate_to_file(FileEntry::Source(path.to_path_buf()), false, false, false, cx)
        });
        viewer.run_until_parked();
        viewer.render();
        viewer
    }

    /// Another open window of the same app.
    pub fn window(&self, id: WindowId) -> Self {
        Self {
            app: self.app.clone(),
            id,
        }
    }

    /// The app's other windows, oldest first.
    pub fn others(&self) -> Vec<Self> {
        self.app
            .windows()
            .into_iter()
            .filter(|id| *id != self.id)
            .map(|id| self.window(id))
            .collect()
    }

    pub fn is_open(&self) -> bool {
        self.app.windows().contains(&self.id)
    }

    pub fn read<R>(&self, f: impl FnOnce(&DataFrameView) -> R) -> R {
        self.app.read(self.id, f)
    }

    /// Changes the view, then handles what its parts reported.
    pub fn update<R>(&self, f: impl FnOnce(&mut DataFrameView, &mut Cx<DataFrameView>) -> R) -> R {
        self.app.update(self.id, |view, cx| {
            let result = f(view, cx);
            view.after_input(cx);
            cx.notify();
            result
        })
    }

    pub fn render(&self) {
        self.app.render(self.id);
    }

    /// Presses each of `keys`, written like `ctrl-shift-p`.
    pub fn press(&self, keys: &str) {
        for key in keys.split_whitespace() {
            self.app.press(self.id, key);
        }
    }

    /// Types `text` into the focused input.
    pub fn input(&self, text: &str) {
        self.app.type_text(self.id, text);
    }

    pub fn run_until_parked(&self) {
        self.app.run_until_parked();
    }

    pub fn advance_clock(&self, by: Duration) {
        self.app.advance_clock(by);
    }

    /// Lets background work, debounces and short timers finish.
    pub fn settle(&self) {
        self.run_until_parked();
        self.advance_clock(Duration::from_millis(50));
        self.run_until_parked();
        self.render();
    }

    pub fn resize(&self, width: f32, height: f32) {
        self.app.resize(self.id, width, height);
        self.render();
    }

    pub fn focus(&self, id: WidgetId) {
        self.update(|view, cx| view.set_focus(Some(id), cx));
    }

    pub fn focused(&self) -> Option<WidgetId> {
        self.read(|view| view.focus)
    }

    pub fn focus_grid(&self) {
        self.update(|view, cx| view.focus_grid(cx));
    }

    pub fn focus_search(&self) {
        let id = self.read(|view| view.search_input.id);
        self.focus(id);
    }

    pub fn grid_focused(&self) -> bool {
        self.read(|view| view.focus == Some(view.grid.id))
    }

    pub fn search_focused(&self) -> bool {
        self.read(|view| view.focus == Some(view.search_input.id))
    }

    /// Moves the grid's cursor to `cell` and focuses the grid.
    pub fn select(&self, row: usize, column: usize) {
        self.update(|view, cx| {
            view.focus_grid(cx);
            view.grid.select(Cell { row, column }, false);
        });
    }

    pub fn has_dialog(&self) -> bool {
        self.read(|view| view.dialog.is_some())
    }

    pub fn close_dialog(&self) {
        self.update(|view, cx| view.close_dialog(cx));
    }

    /// Clicks the gear and switches `option` in the View options list,
    /// leaving the list open.
    pub fn view_option(&self, option: super::view_options::ViewOption) {
        if !self.read(|view| matches!(&view.dialog, Some(super::dialogs::Dialog::Palette(dialog)) if matches!(dialog.kind, super::dialogs::PaletteKind::ViewOptions))) {
            self.click(|hit| *hit == Hit::Button(crate::ui::hit::Button::ViewOptions));
        }
        self.update(|view, cx| {
            let index = view
                .view_options()
                .iter()
                .position(|(row, ..)| *row == option)
                .expect("the option applies here");
            view.confirm_view_option(index, cx);
        });
        self.run_until_parked();
    }

    /// The rectangle of the last frame's top-most region matching `matches`.
    pub fn find(&self, matches: impl Fn(&Hit) -> bool) -> Option<Rect> {
        self.app.find(self.id, matches)
    }

    /// Clicks the middle of the region matching `matches`.
    pub fn click(&self, matches: impl Fn(&Hit) -> bool) {
        self.render();
        let target = self.find(matches).expect("a region to click");
        self.app.click(self.id, target.center());
    }

    pub fn click_at(&self, pos: Point) {
        self.app.click(self.id, pos);
    }

    pub fn drag(&self, from: Point, to: Point) {
        self.app.drag(self.id, from, to);
    }

    /// Presses at `from` with `modifiers` held, moves to `to`, and releases.
    pub fn drag_with(&self, from: Point, to: Point, modifiers: crate::ui::platform::Modifiers) {
        self.render();
        self.app.mouse_down(self.id, from, modifiers, 1);
        self.render();
        if to != from {
            self.app.mouse_move(self.id, to, true);
            self.render();
        }
        self.app.mouse_up(self.id, to);
        self.render();
    }

    pub fn right_click(&self, matches: impl Fn(&Hit) -> bool) {
        self.render();
        let target = self.find(matches).expect("a region to click");
        self.app.right_click(self.id, target.center());
        self.render();
    }

    /// Drops `paths` on the middle of the window, as from File Explorer.
    pub fn drop_files(&self, paths: Vec<std::path::PathBuf>) {
        use crate::ui::platform::{Event, point};
        self.render();
        self.app.dispatch(
            self.id,
            Event::Drop {
                paths,
                pos: point(WIDTH / 2.0, HEIGHT / 2.0),
                hit: None,
            },
        );
        self.run_until_parked();
    }

    /// Asks the window to close, like its close button.
    pub fn request_close(&self) -> bool {
        use crate::ui::platform::View;
        let closes = self.app.update(self.id, |view, cx| view.close_requested(cx));
        if closes {
            self.app.close(self.id);
        }
        closes
    }
}
