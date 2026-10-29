//! The data grid: a virtualized table of the current result with frozen
//! columns, keyboard navigation, rectangular selection, and inferred widths.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::Duration;

use crate::application::keymap::{ActionId, KeyContext, KeymapFile, canonical_keystroke};
use crate::application::theme::ThemeSettings;
use crate::domain::documents::SortKey;
use crate::domain::formatting::{
    NumericFormat, default_numeric_format, display_cell, temporal_type_name, type_label,
};
use crate::domain::frame::{FrameSnapshot, ResultSnapshot};
use crate::domain::layout::GridGeometry;
use crate::domain::selection::{Cell, Selection};
use crate::domain::widths::{FALLBACK_WIDTH, inferred_width};
use crate::ui::hit::{GridHit, Hit};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{
    Align, Canvas, Color, Cursor, Cx, Font, Point, Rect, Task, TextOptions, View, point, rect, text,
};
use crate::ui::widgets::{WidgetId, colors, scroll};

const CELL_PREVIEW_LIMIT: usize = 200;
const SEARCH_HIGHLIGHT: Color = Color::rgba(0xffd84d99);
const TOOLTIP_DELAY: Duration = Duration::from_millis(500);

fn cell_preview(value: Option<&str>, dtype: &str, format: Option<NumericFormat>) -> (String, bool) {
    let text = display_cell(value, dtype, format, CELL_PREVIEW_LIMIT);
    let hidden = text.chars().count() > CELL_PREVIEW_LIMIT || text.contains(['\r', '\n', '\t']);
    (
        text.replace("\r\n", " ").replace(['\r', '\n', '\t'], " "),
        hidden,
    )
}

fn is_grid_navigation_action(action: ActionId) -> bool {
    matches!(
        action,
        ActionId::MoveLeft
            | ActionId::MoveRight
            | ActionId::MoveUp
            | ActionId::MoveDown
            | ActionId::ExtendLeft
            | ActionId::ExtendRight
            | ActionId::ExtendUp
            | ActionId::ExtendDown
            | ActionId::JumpLeft
            | ActionId::JumpRight
            | ActionId::JumpUp
            | ActionId::JumpDown
            | ActionId::ExtendToLeft
            | ActionId::ExtendToRight
            | ActionId::ExtendToUp
            | ActionId::ExtendToDown
            | ActionId::FirstColumn
            | ActionId::LastColumn
            | ActionId::FirstCell
            | ActionId::LastCell
            | ActionId::PreviousPage
            | ActionId::NextPage
            | ActionId::ExtendPreviousPage
            | ActionId::ExtendNextPage
    )
}

/// A view that holds a grid, so the grid's background work can find it.
pub trait GridHost: View<Hit = Hit> {
    fn grid_mut(&mut self) -> &mut GridView;
}

/// A totals row pinned under the rows. It is not part of the result, so
/// selection, search, sort, copy, and export never see it.
#[derive(Clone, Debug, PartialEq)]
pub struct GridFooter {
    /// Shown in the first column when that column has no value.
    pub label: String,
    /// Values by column name; columns not listed stay blank.
    pub values: Vec<(String, Option<String>)>,
}

impl GridFooter {
    fn value(&self, column: &str) -> Option<&Option<String>> {
        self.values
            .iter()
            .find(|(name, _)| name == column)
            .map(|(_, value)| value)
    }
}

struct HeaderCache {
    font: Font,
    labels: Vec<String>,
    height: f32,
}

pub(crate) struct WidthCandidates {
    pub(crate) column: String,
    pub(crate) samples: Vec<String>,
    pub(crate) epoch: u64,
    pub(crate) revision: u64,
}

#[derive(Clone)]
pub enum GridEvent {
    NumericFormatChanged {
        column: String,
        format: NumericFormat,
    },
    WidthChanged {
        owner: String,
        column: String,
        width: f32,
    },
    WidthInferenceFailed {
        owner: String,
        error: String,
    },
    SortRequested(String),
    SortDirectionRequested(bool),
    ClearSortRequested(String),
    CopyRequested {
        result: ResultSnapshot,
        selection: Selection,
    },
    SelectionChanged {
        result: ResultSnapshot,
        selection: Selection,
        /// Moved by a click or key, not kept across a new result.
        by_user: bool,
    },
    SelectionCleared,
    MoveColumn(isize),
    ToggleFrozen,
}

/// The full text of a cell shown in a popup while the pointer rests on it.
pub(crate) struct CellTooltip {
    pub(crate) row: usize,
    pub(crate) column: usize,
    pub(crate) text: String,
    rect: Rect,
    scroll: scroll::Scroll,
    content_height: f32,
}

/// Where the last paint put the grid's parts.
#[derive(Clone, Copy, Default)]
struct Layout {
    bounds: Rect,
    rows: Rect,
    header_height: f32,
    vertical_bar: Rect,
    horizontal_bar: Rect,
}

/// A heat-map cell background: a translucent green, stronger for larger
/// values, so it reads on light and dark themes.
fn heat_color(fraction: f32) -> Color {
    Color::hsla(0.36, 0.6, 0.45, 0.06 + 0.5 * fraction)
}

pub struct GridView {
    pub id: WidgetId,
    result: ResultSnapshot,
    theme: ThemeSettings,
    keymap: KeymapFile,
    key_context: KeyContext,
    selection: Option<Selection>,
    pub(crate) widths: Vec<f32>,
    width_owner: String,
    width_store: usize,
    pub(crate) width_epoch: u64,
    pub(crate) known_widths: BTreeMap<String, f32>,
    width_revisions: HashMap<String, u64>,
    pub(crate) pending_widths: HashSet<String>,
    pub(crate) width_candidates: VecDeque<WidthCandidates>,
    width_batch_blocked: bool,
    row_height: f32,
    typography: Option<Font>,
    formats: Vec<Option<NumericFormat>>,
    frozen_count: usize,
    pub(crate) scroll_x: f32,
    h_scroll: scroll::Scroll,
    v_scroll: scroll::Scroll,
    selection_drag: Option<Cell>,
    sort: Vec<SortKey>,
    pending_pages: HashSet<(u64, usize)>,
    header_cache: Option<HeaderCache>,
    benchmark_rendered_revision: Option<u64>,
    highlights: crate::domain::query::SearchHighlights,
    footer: Option<GridFooter>,
    /// Widen columns to fit the footer once width inference settles.
    footer_fit: bool,
    /// Heat-map ranges by column name; cells of other columns are not shaded.
    heat_ranges: HashMap<String, (f64, f64)>,
    layout: Layout,
    /// Cells drawn cut short in the last frame, which offer their full text.
    overflowing: HashSet<(usize, usize)>,
    hover: Option<(usize, usize)>,
    hover_task: Option<Task>,
    pub(crate) tooltip: Option<CellTooltip>,
    events: Vec<GridEvent>,
}

impl GridView {
    #[cfg(test)]
    pub(crate) fn numeric_format(&self, name: &str) -> Option<NumericFormat> {
        self.result
            .columns
            .iter()
            .position(|column| column.0 == name)
            .and_then(|index| self.formats[index])
    }

    /// Events since the last call, for the host to handle.
    pub fn take_events(&mut self) -> Vec<GridEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn configure_formats(&mut self, saved: &BTreeMap<String, NumericFormat>) {
        self.formats = self
            .result
            .columns
            .iter()
            .zip(self.result.dtypes.iter())
            .map(|(column, dtype)| {
                default_numeric_format(dtype).map(|default| {
                    let mut format = saved.get(&column.0).copied().unwrap_or(default);
                    format.precision = format.precision.min(38);
                    format
                })
            })
            .collect();
    }

    pub fn change_numeric_format(&mut self, action: ActionId) {
        let Some(selection) = self.selection else {
            return;
        };
        let column = selection.active.column;
        let Some(mut format) = self.formats.get(column).copied().flatten() else {
            return;
        };
        let previous = format;
        match action {
            ActionId::ToggleThousandsGrouping => format.grouping = !format.grouping,
            ActionId::IncreaseDecimalPrecision => {
                format.precision = format.precision.saturating_add(1).min(38)
            }
            ActionId::DecreaseDecimalPrecision => {
                format.precision = format.precision.saturating_sub(1)
            }
            _ => return,
        }
        if format != previous {
            self.formats[column] = Some(format);
            self.events.push(GridEvent::NumericFormatChanged {
                column: self.result.columns[column].0.clone(),
                format,
            });
        }
    }

    pub(crate) fn benchmark_scroll_to(&mut self, row: usize) {
        self.v_scroll.offset = row as f32 * self.row_height;
    }

    pub fn new(result: ResultSnapshot) -> Self {
        let widths = vec![FALLBACK_WIDTH; result.columns.len()];
        let formats = result
            .dtypes
            .iter()
            .map(|dtype| default_numeric_format(dtype))
            .collect();
        let selection = if result.row_count() > 0 && result.column_count() > 0 {
            let first = Cell { row: 0, column: 0 };
            Some(Selection {
                anchor: first,
                active: first,
            })
        } else {
            None
        };
        Self {
            id: WidgetId::new(),
            result,
            theme: ThemeSettings::default(),
            keymap: KeymapFile::default(),
            key_context: KeyContext::Grid,
            selection,
            widths,
            width_owner: String::new(),
            width_store: 0,
            width_epoch: 0,
            known_widths: BTreeMap::new(),
            width_revisions: HashMap::new(),
            pending_widths: HashSet::new(),
            width_candidates: VecDeque::new(),
            width_batch_blocked: false,
            row_height: 26.0,
            typography: None,
            formats,
            frozen_count: 0,
            scroll_x: 0.0,
            h_scroll: scroll::Scroll::default(),
            v_scroll: scroll::Scroll::default(),
            selection_drag: None,
            sort: Vec::new(),
            pending_pages: HashSet::new(),
            header_cache: None,
            benchmark_rendered_revision: None,
            highlights: Default::default(),
            footer: None,
            footer_fit: false,
            heat_ranges: Default::default(),
            layout: Layout::default(),
            overflowing: HashSet::new(),
            hover: None,
            hover_task: None,
            tooltip: None,
            events: Vec::new(),
        }
    }

    pub fn set_heat_ranges(&mut self, ranges: HashMap<String, (f64, f64)>) {
        if self.heat_ranges != ranges {
            self.heat_ranges = ranges;
        }
    }

    pub fn set_footer(&mut self, footer: Option<GridFooter>) {
        if self.footer != footer {
            self.footer_fit = footer.is_some();
            self.footer = footer;
        }
    }

    /// The footer text shown in `column`, if any.
    fn footer_text(&self, column: usize) -> Option<String> {
        let footer = self.footer.as_ref()?;
        match footer.value(&self.result.columns.get(column)?.0) {
            Some(value) => Some(
                cell_preview(
                    value.as_deref(),
                    &self.result.dtypes[column],
                    self.formats[column],
                )
                .0,
            ),
            None if column == 0 => Some(footer.label.clone()),
            None => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn footer(&self) -> Option<&GridFooter> {
        self.footer.as_ref()
    }

    pub(crate) fn benchmark_has_rendered_result(&self) -> bool {
        self.benchmark_rendered_revision == Some(self.result.result_revision)
    }

    pub fn result(&self) -> &ResultSnapshot {
        &self.result
    }

    pub fn widths_ready(&self) -> bool {
        self.pending_widths.is_empty() && self.width_candidates.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn column_width(&self, name: &str) -> f32 {
        self.widths[self
            .result
            .columns
            .iter()
            .position(|column| column.0 == name)
            .unwrap()]
    }

    pub fn reset_widths(&mut self) {
        self.width_epoch += 1;
        self.known_widths.clear();
        self.pending_widths.clear();
        self.width_candidates.clear();
        self.width_revisions.clear();
        self.width_batch_blocked = false;
        self.widths.fill(FALLBACK_WIDTH);
        self.header_cache = None;
        self.footer_fit = self.footer.is_some();
    }

    /// Uses `saved` widths and measures the other columns on a worker.
    pub fn configure_widths<V: GridHost>(
        &mut self,
        owner: String,
        source: Option<FrameSnapshot>,
        saved: BTreeMap<String, f32>,
        cx: &mut Cx<V>,
    ) {
        let store = source
            .as_ref()
            .map(|source| source.store.clone())
            .unwrap_or_else(|| self.result.store.clone());
        let store_key = std::sync::Arc::as_ptr(&store) as usize;
        if self.width_owner != owner {
            self.reset_widths();
            self.width_owner = owner;
        } else if self.width_store != store_key {
            self.width_epoch += 1;
            self.pending_widths.clear();
            self.width_candidates.clear();
        }
        self.width_store = store_key;
        self.known_widths.extend(
            saved
                .into_iter()
                .filter(|(_, width)| crate::domain::table_preferences::valid_width(*width)),
        );
        self.widths = self
            .result
            .columns
            .iter()
            .map(|column| {
                self.known_widths
                    .get(&column.0)
                    .copied()
                    .unwrap_or(FALLBACK_WIDTH)
            })
            .collect();
        self.header_cache = None;
        let columns = self
            .result
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| !self.known_widths.contains_key(&column.0))
            .filter(|(_, column)| self.pending_widths.insert(column.0.clone()))
            .map(|(index, column)| (column.0.clone(), self.formats[index]))
            .collect::<Vec<_>>();
        if columns.is_empty() {
            cx.notify();
            return;
        }
        let epoch = self.width_epoch;
        let revisions = columns
            .iter()
            .map(|(name, _)| {
                (
                    name.clone(),
                    self.width_revisions.get(name).copied().unwrap_or(0),
                )
            })
            .collect::<HashMap<_, _>>();
        let rows = source.is_none().then(|| self.result.row_ids.clone());
        cx.spawn(
            move || {
                store
                    .lock()
                    .map_err(|_| "Width sampling store lock poisoned".to_owned())?
                    .width_candidates(rows.as_ref(), &columns)
            },
            move |host: &mut V, cx, result: Result<Vec<(String, Vec<String>)>, String>| {
                let grid = host.grid_mut();
                if grid.width_epoch != epoch {
                    return;
                }
                match result {
                    Ok(candidates) => {
                        for (column, samples) in candidates {
                            if grid.width_revisions.get(&column).copied().unwrap_or(0)
                                == revisions[&column]
                                && !grid.known_widths.contains_key(&column)
                            {
                                grid.width_candidates.push_back(WidthCandidates {
                                    revision: revisions[&column],
                                    column,
                                    samples,
                                    epoch,
                                });
                            } else {
                                grid.pending_widths.remove(&column);
                            }
                        }
                    }
                    Err(error) => {
                        for column in revisions.keys() {
                            grid.pending_widths.remove(column);
                        }
                        let owner = grid.width_owner.clone();
                        grid.events
                            .push(GridEvent::WidthInferenceFailed { owner, error });
                    }
                }
                cx.notify();
            },
        )
        .detach();
        cx.notify();
    }

    fn change_width(&mut self, increase: bool) {
        let Some(selection) = self.selection else {
            return;
        };
        let column = selection.active.column;
        let width = (self.widths[column] + if increase { 10.0 } else { -10.0 }).clamp(40.0, 1200.0);
        if width == self.widths[column] {
            return;
        }
        let name = self.result.columns[column].0.clone();
        *self.width_revisions.entry(name.clone()).or_default() += 1;
        self.pending_widths.remove(&name);
        self.known_widths.insert(name.clone(), width);
        self.widths[column] = width;
        self.header_cache = None;
        self.keep_active_column_visible(self.viewport_width());
        self.events.push(GridEvent::WidthChanged {
            owner: self.width_owner.clone(),
            column: name,
            width,
        });
    }

    fn viewport_width(&self) -> f32 {
        let width = self.layout.rows.w;
        if width > 0.0 { width } else { 1400.0 }
    }

    fn keep_active_column_visible(&mut self, viewport_width: f32) {
        let geometry = GridGeometry::new(
            self.widths.clone(),
            self.frozen_count,
            viewport_width,
            self.scroll_x,
        );
        if let Some(selection) = self.selection
            && selection.active.column >= self.frozen_count
        {
            let column = selection.active.column;
            if let Some(x) = geometry.column_x(column) {
                if self.widths[column] > geometry.scrollable_width() {
                    self.scroll_x += x - geometry.frozen_width();
                } else if x < geometry.frozen_width() {
                    self.scroll_x -= geometry.frozen_width() - x;
                } else if x + self.widths[column] > viewport_width {
                    self.scroll_x += x + self.widths[column] - viewport_width;
                }
            }
        }
        self.scroll_x = self.scroll_x.clamp(0.0, geometry.max_scroll_x());
    }

    pub fn set_theme(&mut self, theme: ThemeSettings) {
        self.theme = theme;
    }

    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    pub fn set_keymap(&mut self, keymap: KeymapFile, context: KeyContext) {
        self.keymap = keymap;
        self.key_context = context;
    }

    pub fn set_sort(&mut self, sort: Vec<SortKey>) {
        self.sort = sort;
        self.header_cache = None;
    }

    /// Search words to highlight in cells.
    pub fn set_highlights(&mut self, highlights: crate::domain::query::SearchHighlights) {
        if self.highlights != highlights {
            self.highlights = highlights;
        }
    }

    /// Scrolls so `row` shows, moving as little as possible.
    fn scroll_to_row(&mut self, row: usize) {
        let viewport = self.layout.rows.h.max(self.row_height);
        self.v_scroll
            .reveal(row as f32 * self.row_height, self.row_height, viewport);
    }

    /// Moves the active cell to `column`, keeping its row, and scrolls it into view.
    pub fn jump_to_column(&mut self, column: usize) {
        if column >= self.result.column_count() || self.result.row_count() == 0 {
            return;
        }
        let row = self
            .selection
            .map_or(0, |selection| selection.active.row)
            .min(self.result.row_count() - 1);
        self.select(Cell { row, column }, false);
        self.scroll_to_row(row);
        self.keep_active_column_visible(self.viewport_width());
    }

    pub fn frozen_count(&self) -> usize {
        self.frozen_count
    }

    pub fn set_frozen_count(&mut self, count: usize) {
        self.frozen_count = count.min(self.result.column_count());
    }

    pub fn replace_result(&mut self, result: ResultSnapshot) {
        let old_widths = self
            .result
            .columns
            .iter()
            .zip(&self.widths)
            .map(|(column, width)| (column.0.as_str(), *width))
            .collect::<HashMap<_, _>>();
        self.widths = result
            .columns
            .iter()
            .map(|column| {
                old_widths
                    .get(column.0.as_str())
                    .copied()
                    .unwrap_or(FALLBACK_WIDTH)
            })
            .collect();
        self.formats = result
            .dtypes
            .iter()
            .map(|dtype| default_numeric_format(dtype))
            .collect();
        self.frozen_count = self.frozen_count.min(result.columns.len());
        self.selection = self
            .selection
            .and_then(|selection| {
                let map_column = |old: usize| {
                    self.result
                        .columns
                        .get(old)
                        .and_then(|old| result.columns.iter().position(|new| new == old))
                        .unwrap_or_else(|| old.min(result.columns.len().saturating_sub(1)))
                };
                Selection {
                    anchor: Cell {
                        row: selection.anchor.row,
                        column: map_column(selection.anchor.column),
                    },
                    active: Cell {
                        row: selection.active.row,
                        column: map_column(selection.active.column),
                    },
                }
                .clamped(result.row_count(), result.column_count())
            })
            .or_else(|| {
                if result.row_count() > 0 && result.column_count() > 0 {
                    let first = Cell { row: 0, column: 0 };
                    Some(Selection {
                        anchor: first,
                        active: first,
                    })
                } else {
                    None
                }
            });
        self.result = result;
        self.benchmark_rendered_revision = None;
        self.pending_pages.clear();
        self.scroll_x = 0.0;
        self.header_cache = None;
        self.v_scroll.offset = 0.0;
        self.tooltip = None;
        self.hover = None;
        self.hover_task = None;
        if let Some(selection) = self.selection {
            self.events.push(GridEvent::SelectionChanged {
                result: self.result.clone(),
                selection,
                by_user: false,
            });
        } else {
            self.events.push(GridEvent::SelectionCleared);
        }
    }

    pub fn select(&mut self, cell: Cell, extend: bool) {
        self.select_cell(cell, extend, true);
    }

    fn select_cell(&mut self, cell: Cell, extend: bool, by_user: bool) {
        if cell.row >= self.result.row_count() || cell.column >= self.result.column_count() {
            return;
        }
        self.selection = Some(if extend {
            Selection {
                anchor: self
                    .selection
                    .map(|selection| selection.anchor)
                    .unwrap_or(cell),
                active: cell,
            }
        } else {
            Selection {
                anchor: cell,
                active: cell,
            }
        });
        self.events.push(GridEvent::SelectionChanged {
            result: self.result.clone(),
            selection: self.selection.unwrap(),
            by_user,
        });
    }

    pub fn reset_navigation_position(&mut self) {
        self.selection = None;
        self.scroll_x = 0.0;
        self.v_scroll.offset = 0.0;
        if self.result.row_count() > 0 && self.result.column_count() > 0 {
            self.select_cell(Cell { row: 0, column: 0 }, false, false);
        } else {
            self.events.push(GridEvent::SelectionCleared);
        }
    }

    fn begin_selection_drag(&mut self, cell: Cell, extend: bool) {
        self.selection_drag = Some(if extend {
            self.selection
                .map(|selection| selection.anchor)
                .unwrap_or(cell)
        } else {
            cell
        });
        self.select(cell, extend);
    }

    fn extend_selection_drag(&mut self, position: Point) {
        let Some(anchor) = self.selection_drag else {
            return;
        };
        let bounds = self.layout.rows;
        if bounds.w <= 0.0 || bounds.h <= 0.0 {
            return;
        }
        let x = (position.x - bounds.x).clamp(0.0, bounds.w - 0.01);
        let y = (position.y - bounds.y).clamp(0.0, bounds.h - 0.01);
        let geometry = GridGeometry::new(self.widths.clone(), self.frozen_count, bounds.w, self.scroll_x);
        let Some(column) = geometry.hit_test_column(x) else {
            return;
        };
        let row = ((y + self.v_scroll.offset) / self.row_height).floor().max(0.0) as usize;
        let active = Cell {
            row: row.min(self.result.row_count().saturating_sub(1)),
            column,
        };
        if self
            .selection
            .is_some_and(|selection| selection.anchor == anchor && selection.active == active)
        {
            return;
        }
        self.selection = Some(Selection { anchor, active });
        self.events.push(GridEvent::SelectionChanged {
            result: self.result.clone(),
            selection: self.selection.unwrap(),
            by_user: true,
        });
    }

    pub fn scroll_horizontal(&mut self, delta: f32, viewport_width: f32) {
        let geometry = GridGeometry::new(
            self.widths.clone(),
            self.frozen_count,
            viewport_width,
            self.scroll_x,
        );
        self.scroll_x = (self.scroll_x + delta).clamp(0.0, geometry.max_scroll_x());
    }

    /// Moves the active cell for a navigation action. `window_height` sizes a page.
    pub fn navigate_action(&mut self, action: ActionId, window_height: f32) {
        let rows = self.result.row_count();
        let columns = self.result.column_count();
        if rows == 0 || columns == 0 {
            return;
        }
        let current = self
            .selection
            .map(|selection| selection.active)
            .unwrap_or(Cell { row: 0, column: 0 });
        let mut next = current;
        let page = ((window_height - 120.0) / self.row_height).floor().max(1.0) as usize;
        match action {
            ActionId::MoveLeft => next.column = next.column.saturating_sub(1),
            ActionId::MoveRight => next.column = (next.column + 1).min(columns - 1),
            ActionId::MoveUp => next.row = next.row.saturating_sub(1),
            ActionId::MoveDown => next.row = (next.row + 1).min(rows - 1),
            ActionId::JumpLeft | ActionId::ExtendToLeft => next.column = 0,
            ActionId::JumpRight | ActionId::ExtendToRight => next.column = columns - 1,
            ActionId::JumpUp | ActionId::ExtendToUp => next.row = 0,
            ActionId::JumpDown | ActionId::ExtendToDown => next.row = rows - 1,
            ActionId::ExtendLeft => next.column = next.column.saturating_sub(1),
            ActionId::ExtendRight => next.column = (next.column + 1).min(columns - 1),
            ActionId::ExtendUp => next.row = next.row.saturating_sub(1),
            ActionId::ExtendDown => next.row = (next.row + 1).min(rows - 1),
            ActionId::FirstColumn => next.column = 0,
            ActionId::LastColumn => next.column = columns - 1,
            ActionId::FirstCell => {
                next.row = 0;
                next.column = 0;
            }
            ActionId::LastCell => {
                next.row = rows - 1;
                next.column = columns - 1;
            }
            ActionId::PreviousPage | ActionId::ExtendPreviousPage => {
                next.row = next.row.saturating_sub(page)
            }
            ActionId::NextPage | ActionId::ExtendNextPage => {
                next.row = next.row.saturating_add(page).min(rows - 1)
            }
            _ => return,
        }
        let extend = matches!(
            action,
            ActionId::ExtendLeft
                | ActionId::ExtendRight
                | ActionId::ExtendUp
                | ActionId::ExtendDown
                | ActionId::ExtendToLeft
                | ActionId::ExtendToRight
                | ActionId::ExtendToUp
                | ActionId::ExtendToDown
                | ActionId::ExtendPreviousPage
                | ActionId::ExtendNextPage
        );
        self.selection = Some(if extend {
            Selection {
                anchor: self
                    .selection
                    .map(|selection| selection.anchor)
                    .unwrap_or(current),
                active: next,
            }
        } else {
            Selection {
                anchor: next,
                active: next,
            }
        });
        self.events.push(GridEvent::SelectionChanged {
            result: self.result.clone(),
            selection: self.selection.unwrap(),
            by_user: true,
        });
        self.scroll_to_row(next.row);
        self.keep_active_column_visible(self.viewport_width());
    }

    /// The grid's own keys. Returns whether the press was used.
    pub fn key(&mut self, event: &KeyEvent, window_height: f32) -> bool {
        let Ok(stroke) = canonical_keystroke(&event.stroke()) else {
            return false;
        };
        // Shifted punctuation arrives as Shift and the unshifted key; match the
        // produced character as well as explicitly rebound physical keystrokes.
        let modifiers = event.modifiers;
        let produced = if !modifiers.control && !modifiers.alt && !modifiers.platform {
            event
                .key_char
                .as_deref()
                .filter(|key| matches!(*key, "(" | ")" | "<" | ">"))
                .or(match (modifiers.shift, event.key.as_str()) {
                    (true, "0" | ")") => Some(")"),
                    (true, "9" | "(") => Some("("),
                    (true, "," | "<") => Some("<"),
                    (true, "." | ">") => Some(">"),
                    _ => None,
                })
        } else {
            None
        };
        let action = self
            .keymap
            .binding_for(&stroke, self.key_context)
            .or_else(|| produced.and_then(|key| self.keymap.binding_for(key, self.key_context)));
        let Some(action) = action else {
            return false;
        };
        if action == ActionId::IncreaseColumnWidth || action == ActionId::DecreaseColumnWidth {
            self.change_width(action == ActionId::IncreaseColumnWidth);
        } else if matches!(
            action,
            ActionId::ToggleThousandsGrouping
                | ActionId::IncreaseDecimalPrecision
                | ActionId::DecreaseDecimalPrecision
        ) {
            self.change_numeric_format(action);
        } else if action == ActionId::MoveColumnLeft {
            self.events.push(GridEvent::MoveColumn(-1));
        } else if action == ActionId::MoveColumnRight {
            self.events.push(GridEvent::MoveColumn(1));
        } else if action == ActionId::SortAscending {
            self.events.push(GridEvent::SortDirectionRequested(false));
        } else if action == ActionId::SortDescending {
            self.events.push(GridEvent::SortDirectionRequested(true));
        } else if is_grid_navigation_action(action) {
            self.navigate_action(action, window_height);
        } else {
            return false;
        }
        true
    }

    /// A press on the grid. Returns whether the grid should take focus.
    pub fn mouse_down(&mut self, hit: GridHit, pos: Point, shift: bool, right: bool) -> bool {
        match hit {
            GridHit::Header(column) => {
                if let Some(name) = self.result.columns.get(column).map(|column| column.0.clone()) {
                    self.events.push(if right {
                        GridEvent::ClearSortRequested(name)
                    } else {
                        GridEvent::SortRequested(name)
                    });
                }
                false
            }
            GridHit::Cell { row, column } if !right => {
                self.begin_selection_drag(Cell { row, column }, shift);
                true
            }
            GridHit::VerticalBar => {
                let bar = self.layout.vertical_bar;
                let content = self.result.row_count() as f32 * self.row_height;
                self.v_scroll
                    .press(pos.y - bar.y, bar.h, self.layout.rows.h, content);
                true
            }
            GridHit::HorizontalBar => {
                let bar = self.layout.horizontal_bar;
                let geometry = GridGeometry::new(self.widths.clone(), self.frozen_count, self.layout.rows.w, self.scroll_x);
                self.h_scroll.offset = self.scroll_x;
                self.h_scroll.press(
                    pos.x - bar.x,
                    bar.w,
                    geometry.scrollable_width(),
                    geometry.content_width(),
                );
                self.scroll_x = self.h_scroll.offset;
                true
            }
            GridHit::TooltipBar => {
                if let Some(tooltip) = &mut self.tooltip {
                    let bar = rect(tooltip.rect.right() - scroll::BAR_WIDTH, tooltip.rect.y, scroll::BAR_WIDTH, tooltip.rect.h);
                    tooltip
                        .scroll
                        .press(pos.y - bar.y, bar.h, tooltip.rect.h - 16.0, tooltip.content_height);
                }
                false
            }
            _ => !right,
        }
    }

    /// The pointer moved with the button down after a press on the grid.
    pub fn mouse_drag(&mut self, pos: Point) {
        if self.v_scroll.dragging() {
            let bar = self.layout.vertical_bar;
            let content = self.result.row_count() as f32 * self.row_height;
            self.v_scroll
                .drag(pos.y - bar.y, bar.h, self.layout.rows.h, content);
        } else if self.h_scroll.dragging() {
            let bar = self.layout.horizontal_bar;
            let geometry = GridGeometry::new(self.widths.clone(), self.frozen_count, self.layout.rows.w, self.scroll_x);
            self.h_scroll.drag(
                pos.x - bar.x,
                bar.w,
                geometry.scrollable_width(),
                geometry.content_width(),
            );
            self.scroll_x = self.h_scroll.offset;
        } else if let Some(tooltip) = &mut self.tooltip
            && tooltip.scroll.dragging()
        {
            let bar = rect(tooltip.rect.right() - scroll::BAR_WIDTH, tooltip.rect.y, scroll::BAR_WIDTH, tooltip.rect.h);
            tooltip
                .scroll
                .drag(pos.y - bar.y, bar.h, tooltip.rect.h - 16.0, tooltip.content_height);
        } else {
            self.extend_selection_drag(pos);
        }
    }

    pub fn mouse_up(&mut self) {
        self.selection_drag = None;
        self.v_scroll.release();
        self.h_scroll.release();
        if let Some(tooltip) = &mut self.tooltip {
            tooltip.scroll.release();
        }
    }

    /// Wheel movement in lines over the grid. Shift turns it sideways.
    pub fn wheel(&mut self, hit: Option<GridHit>, delta: Point, shift: bool) -> bool {
        if matches!(hit, Some(GridHit::Tooltip | GridHit::TooltipBar))
            && let Some(tooltip) = &mut self.tooltip
        {
            let line = text::line_height(self.typography.as_ref().unwrap_or(&Font::new("Segoe UI", 13.0)));
            return tooltip
                .scroll
                .scroll_by(-delta.y * line, tooltip.rect.h - 16.0, tooltip.content_height);
        }
        let horizontal = if shift { delta.y } else { delta.x };
        if horizontal != 0.0 {
            self.scroll_horizontal(-horizontal * self.row_height, self.viewport_width());
            return true;
        }
        let content = self.result.row_count() as f32 * self.row_height;
        self.v_scroll
            .scroll_by(-delta.y * self.row_height, self.layout.rows.h, content)
    }

    /// Tracks the hovered cell, showing its full text after a pause when it
    /// is cut short.
    pub fn mouse_move<V: GridHost>(&mut self, hit: Option<GridHit>, cx: &mut Cx<V>) {
        let cell = match hit {
            Some(GridHit::Cell { row, column }) => Some((row, column)),
            Some(GridHit::Tooltip | GridHit::TooltipBar) => {
                // The pointer moved onto the open tooltip, which stays.
                return;
            }
            _ => None,
        };
        if cell == self.hover {
            return;
        }
        self.hover = cell;
        if self
            .tooltip
            .as_ref()
            .is_some_and(|tooltip| Some((tooltip.row, tooltip.column)) != cell)
        {
            self.tooltip = None;
            cx.notify();
        }
        self.hover_task = None;
        let Some((row, column)) = cell.filter(|cell| self.overflowing.contains(cell)) else {
            return;
        };
        let revision = self.result.result_revision;
        self.hover_task = Some(cx.timer(TOOLTIP_DELAY, move |host: &mut V, cx| {
            let grid = host.grid_mut();
            if grid.hover == Some((row, column)) && grid.result.result_revision == revision {
                grid.show_tooltip(row, column);
                cx.notify();
            }
        }));
    }

    pub fn mouse_leave(&mut self) {
        self.hover = None;
        self.hover_task = None;
        self.tooltip = None;
    }

    fn show_tooltip(&mut self, row: usize, column: usize) {
        let Some(rows) = self.result.cached_rows(row, 1) else {
            return;
        };
        let value = rows
            .first()
            .and_then(|cached| cached.values.get(self.result.column_indices[column]))
            .and_then(Option::as_deref);
        let text = display_cell(value, &self.result.dtypes[column], self.formats[column], usize::MAX);
        self.tooltip = Some(CellTooltip {
            row,
            column,
            text,
            rect: Rect::default(),
            scroll: scroll::Scroll::default(),
            content_height: 0.0,
        });
    }

    fn prefetch_page<V: GridHost>(&mut self, page_index: usize, cx: &mut Cx<V>) {
        let key = (self.result.result_revision, page_index);
        if self.result.page_is_cached(page_index) || !self.pending_pages.insert(key) {
            return;
        }
        let result = self.result.clone();
        cx.spawn(
            move || result.prefetch_page(page_index),
            move |host: &mut V, cx, fetched: Result<(), String>| {
                let grid = host.grid_mut();
                // A failed page stays pending until the result is replaced; retrying
                // on every frame would loop the same failing query.
                if fetched.is_err() {
                    return;
                }
                grid.pending_pages.remove(&key);
                if grid.result.result_revision == key.0 {
                    cx.notify();
                }
            },
        )
        .detach();
    }

    /// Measures up to four columns' samples, as each frame would.
    pub(crate) fn infer_widths<V: GridHost>(&mut self, font: &Font, cx: &mut Cx<V>) {
        let mut inferred = false;
        let mut processed_candidates = false;
        let dtype_font = font.sized((font.size * 0.75).round());
        for _ in 0..if self.width_batch_blocked { 0 } else { 4 } {
            let Some(candidate) = self.width_candidates.pop_front() else {
                break;
            };
            processed_candidates = true;
            if candidate.epoch != self.width_epoch
                || self
                    .width_revisions
                    .get(&candidate.column)
                    .copied()
                    .unwrap_or(0)
                    != candidate.revision
                || self.known_widths.contains_key(&candidate.column)
            {
                continue;
            }
            self.pending_widths.remove(&candidate.column);
            let Some(column) = self
                .result
                .columns
                .iter()
                .position(|column| column.0 == candidate.column)
            else {
                continue;
            };
            let measure = |text: &str, font: &Font| {
                let width = text::measure(text, font).0;
                if width.is_finite() { width } else { f32::NAN }
            };
            let name_width = measure(&candidate.column, font);
            let dtype_width = measure(&type_label(&self.result.dtypes[column]), &dtype_font);
            let mut measured = if name_width.is_finite() && dtype_width.is_finite() {
                name_width.max(dtype_width)
            } else {
                f32::NAN
            };
            for text in &candidate.samples {
                if !measured.is_finite() {
                    break;
                }
                let width = measure(text, font);
                if !width.is_finite() {
                    measured = f32::NAN;
                    break;
                }
                measured = measured.max(width);
            }
            let width = inferred_width(measured);
            self.widths[column] = width;
            self.known_widths.insert(candidate.column.clone(), width);
            self.events.push(GridEvent::WidthChanged {
                owner: self.width_owner.clone(),
                column: candidate.column,
                width,
            });
            inferred = true;
        }
        if inferred {
            self.footer_fit = self.footer.is_some();
        }
        // Totals can be wider than every row, so widen columns to fit them once
        // the row-based widths are known.
        if self.footer_fit && self.pending_widths.is_empty() && self.width_candidates.is_empty() {
            self.footer_fit = false;
            for column in 0..self.result.column_count() {
                let Some(text) = self.footer_text(column) else {
                    continue;
                };
                let width = inferred_width(text::measure(&text, font).0);
                if width > self.widths[column] {
                    self.widths[column] = width;
                    let name = self.result.columns[column].0.clone();
                    self.known_widths.insert(name.clone(), width);
                    self.events.push(GridEvent::WidthChanged {
                        owner: self.width_owner.clone(),
                        column: name,
                        width,
                    });
                    inferred = true;
                }
            }
        }
        if inferred {
            self.header_cache = None;
            self.keep_active_column_visible(self.viewport_width());
        }
        if processed_candidates {
            self.width_batch_blocked = true;
            cx.timer(Duration::ZERO, |host: &mut V, cx| {
                let grid = host.grid_mut();
                grid.width_batch_blocked = false;
                if !grid.width_candidates.is_empty() {
                    cx.notify();
                }
            })
            .detach();
        }
    }

    fn header_height(&mut self, font: &Font) -> f32 {
        if self
            .header_cache
            .as_ref()
            .is_some_and(|cache| cache.font == *font)
        {
            return self.header_cache.as_ref().unwrap().height;
        }
        let dtype_font = font.sized((font.size * 0.75).round());
        let line_height = text::line_height(font);
        let dtype_line_height = text::line_height(&dtype_font);
        let labels = self
            .result
            .columns
            .iter()
            .map(|column| {
                let sort = self
                    .sort
                    .iter()
                    .enumerate()
                    .find(|(_, key)| key.column == column.0)
                    .map(|(priority, key)| {
                        format!(
                            " {}{}",
                            if key.descending { "↓" } else { "↑" },
                            priority + 1
                        )
                    })
                    .unwrap_or_default();
                format!("{}{}", column.0, sort)
            })
            .collect::<Vec<_>>();
        let height = (0..self.result.columns.len())
            .map(|column| {
                let width = (self.widths[column] - 17.0).max(1.0);
                let dtype = type_label(&self.result.dtypes[column]);
                let label_lines = text::layout(&labels[column], font, TextOptions::wrapped(width)).line_count.max(1);
                let dtype_lines = text::layout(&dtype, &dtype_font, TextOptions::wrapped(width)).line_count.max(1);
                9.0 + label_lines as f32 * line_height + dtype_lines as f32 * dtype_line_height
            })
            .fold(48.0_f32, f32::max)
            .ceil();
        self.header_cache = Some(HeaderCache {
            font: font.clone(),
            labels,
            height,
        });
        height
    }

    pub fn paint<V: GridHost>(&mut self, canvas: &mut Canvas<Hit>, bounds: Rect, font: &Font, cx: &mut Cx<V>) {
        let _render = crate::infrastructure::performance::Span::new("grid_render");
        if self.result.row_count() == 0 {
            self.benchmark_rendered_revision = Some(self.result.result_revision);
        }
        let line_height = text::line_height(font);
        let row_height = (line_height + 6.0).ceil().max(26.0);
        if self
            .typography
            .as_ref()
            .is_some_and(|previous| previous != font)
        {
            self.v_scroll.offset *= row_height / self.row_height;
            self.row_height = row_height;
            if let Some(selection) = self.selection {
                self.scroll_to_row(selection.active.row);
            }
            self.header_cache = None;
        }
        self.typography = Some(font.clone());
        self.row_height = row_height;
        self.layout.bounds = bounds;
        self.infer_widths(font, cx);
        let header_height = self.header_height(font);
        let footer_height = if self.footer.is_some() { row_height } else { 0.0 };
        let bar = scroll::BAR_WIDTH;
        let row_count = self.result.row_count();
        // The rows area leaves room for the scroll bars.
        let rows_area = rect(
            bounds.x,
            bounds.y + header_height,
            (bounds.w - if row_count > 0 { bar } else { 0.0 }).max(0.0),
            (bounds.h - header_height - footer_height).max(0.0),
        );
        let geometry_probe = GridGeometry::new(self.widths.clone(), self.frozen_count, rows_area.w, self.scroll_x);
        let horizontal = geometry_probe.max_scroll_x() > 0.0;
        let rows_area = if horizontal {
            rect(rows_area.x, rows_area.y, rows_area.w, (rows_area.h - bar).max(0.0))
        } else {
            rows_area
        };
        self.layout.rows = rows_area;
        self.layout.header_height = header_height;
        let viewport_width = rows_area.w;
        self.scroll_x = self.scroll_x.clamp(0.0, GridGeometry::new(self.widths.clone(), self.frozen_count, viewport_width, 0.0).max_scroll_x());
        let geometry = GridGeometry::new(
            self.widths.clone(),
            self.frozen_count,
            viewport_width,
            self.scroll_x,
        );
        let content_height = row_count as f32 * row_height;
        self.v_scroll.clamp(rows_area.h, content_height);
        let (frozen, scrollable) = geometry.visible_columns(180.0);
        let visible = scrollable.chain(frozen).collect::<Vec<_>>();
        let frozen_count = self.frozen_count;
        let frozen_width = geometry.frozen_width();
        let theme = &self.theme;
        let color = |value: crate::application::theme::ThemeColor| Color::rgba(value.rgba());
        let background = color(theme.background);
        let foreground = color(theme.foreground);
        let head = color(theme.header_background);
        let frozen_head = color(theme.frozen_header_background);
        let even = color(theme.row_stripe_background);
        let active = color(theme.selection_background);
        let active_border = color(theme.active_cell_border);
        let grid_border = color(theme.grid_border);
        let frozen_background = color(theme.frozen_background);
        let frozen_stripe = color(theme.frozen_stripe_background);
        canvas.fill(bounds, background);
        let first_visible_row = (self.v_scroll.offset / row_height) as usize;
        let first_page = first_visible_row / crate::domain::frame::PAGE_SIZE;
        let page_count = row_count.div_ceil(crate::domain::frame::PAGE_SIZE);
        for page in first_page..(first_page + 2).min(page_count) {
            self.prefetch_page(page, cx);
        }
        // Columns left of the frozen edge stay put; others move with scroll_x.
        let column_left = |column: usize, x: f32| {
            if column < frozen_count {
                bounds.x + x
            } else {
                bounds.x + x
            }
        };
        let scroll_clip = rect(bounds.x + frozen_width, bounds.y, (viewport_width - frozen_width).max(0.0), bounds.h);
        // Header.
        let header = rect(bounds.x, bounds.y, bounds.w, header_height);
        canvas.fill(header, head);
        let dtype_font = font.sized((font.size * 0.75).round());
        let labels = self
            .header_cache
            .as_ref()
            .map(|cache| cache.labels.clone())
            .unwrap_or_default();
        for pass_frozen in [false, true] {
            if !pass_frozen {
                canvas.push_clip(rect(scroll_clip.x, header.y, scroll_clip.w, header.h));
            }
            for &column in &visible {
                let is_frozen = column < frozen_count;
                if is_frozen != pass_frozen {
                    continue;
                }
                let Some(x) = geometry.column_x(column) else {
                    continue;
                };
                let cell = rect(column_left(column, x), header.y, self.widths[column], header_height);
                if is_frozen {
                    canvas.fill(cell, frozen_head);
                }
                canvas.fill(rect(cell.right() - 1.0, cell.y, 1.0, cell.h), grid_border);
                let inner = rect(cell.x + 8.0, cell.y + 4.0, (cell.w - 17.0).max(1.0), cell.h - 8.0);
                let label = labels.get(column).cloned().unwrap_or_default();
                let label_layout = text::layout(&label, font, TextOptions::wrapped(inner.w));
                canvas.push_clip(cell);
                canvas.draw_layout(&label_layout, inner.origin(), foreground);
                let dtype = type_label(&self.result.dtypes[column]);
                let dtype_layout = text::layout(&dtype, &dtype_font, TextOptions::wrapped(inner.w));
                canvas.draw_layout(&dtype_layout, point(inner.x, inner.y + label_layout.height), foreground);
                canvas.pop_clip();
                canvas.hit_cursor(cell, Hit::Grid(GridHit::Header(column)), Cursor::Hand);
            }
            if !pass_frozen {
                canvas.pop_clip();
            }
        }
        canvas.fill(rect(bounds.x, header.bottom() - 1.0, bounds.w, 1.0), grid_border);
        // Rows.
        self.overflowing.clear();
        canvas.hit(rows_area, Hit::Grid(GridHit::Body));
        if row_count == 0 {
            canvas.text(
                rect(rows_area.x + 12.0, rows_area.y + 8.0, (rows_area.w - 24.0).max(0.0), line_height + 8.0),
                "No rows to display",
                font,
                foreground,
                Align::Left,
            );
        } else {
            let first_row = first_visible_row.min(row_count - 1);
            let visible_rows = ((rows_area.h / row_height).ceil() as usize + 1).min(row_count - first_row);
            let page_rows = self
                .result
                .cached_rows(first_row, visible_rows)
                .unwrap_or_default();
            if crate::infrastructure::performance::enabled() && page_rows.len() == visible_rows {
                self.benchmark_rendered_revision = Some(self.result.result_revision);
            }
            let heat = self
                .result
                .columns
                .iter()
                .map(|column| self.heat_ranges.get(&column.0).copied())
                .collect::<Vec<_>>();
            let highlight_terms: Vec<Vec<String>> = if self.highlights.is_empty() {
                Vec::new()
            } else {
                let names = self
                    .result
                    .columns
                    .iter()
                    .map(|column| column.0.as_str())
                    .collect::<Vec<_>>();
                self.highlights.for_columns(&names)
            };
            let selection = self.selection;
            let rectangle = selection.map(|selection| selection.rectangle());
            let dtypes = self.result.dtypes.clone();
            let column_indices = self.result.column_indices.clone();
            canvas.push_clip(rows_area);
            for offset in 0..visible_rows {
                let row = first_row + offset;
                let y = rows_area.y + row as f32 * row_height - self.v_scroll.offset;
                if row % 2 == 1 {
                    canvas.fill(rect(rows_area.x, y, rows_area.w, row_height), even);
                }
                for pass_frozen in [false, true] {
                    if !pass_frozen {
                        canvas.push_clip(rect(scroll_clip.x, rows_area.y, scroll_clip.w, rows_area.h));
                    }
                    for &column in &visible {
                        let is_frozen = column < frozen_count;
                        if is_frozen != pass_frozen {
                            continue;
                        }
                        let Some(x) = geometry.column_x(column) else {
                            continue;
                        };
                        let cell = rect(column_left(column, x), y, self.widths[column], row_height);
                        let value = page_rows
                            .get(offset)
                            .and_then(|cached| cached.values.get(column_indices[column]))
                            .and_then(Option::as_deref);
                        let (text, hidden) = cell_preview(value, &dtypes[column], self.formats[column]);
                        let selected = rectangle
                            .as_ref()
                            .is_some_and(|(rows, columns)| rows.contains(&row) && columns.contains(&column));
                        let active_cell = selection.is_some_and(|selection| selection.active == Cell { row, column });
                        if is_frozen {
                            canvas.fill(cell, if row % 2 == 1 { frozen_stripe } else { frozen_background });
                        }
                        if let Some(range) = heat[column]
                            && let Some(number) = value.and_then(|value| value.parse::<f64>().ok())
                        {
                            canvas.fill(cell, heat_color(crate::domain::heatmap::fraction(number, range)));
                        }
                        if selected {
                            canvas.fill(cell, active);
                        }
                        canvas.fill(rect(cell.right() - 1.0, cell.y, 1.0, cell.h), grid_border);
                        let is_numeric = self.formats[column].is_some();
                        let align = if is_numeric {
                            Align::Right
                        } else if temporal_type_name(&dtypes[column]) {
                            Align::Center
                        } else {
                            Align::Left
                        };
                        let available = (cell.w - font.size - if active_cell { 4.0 } else { 1.0 }).max(1.0);
                        let text_area = rect(cell.x + 8.0, cell.y, available, cell.h);
                        let layout = text::layout(&text, font, TextOptions::truncated(available).align(align));
                        let origin = point(text_area.x, cell.y + ((row_height - layout.height) / 2.0).max(0.0));
                        canvas.push_clip(rect(cell.x, cell.y, cell.w - 1.0, cell.h));
                        if let Some(terms) = highlight_terms.get(column)
                            && !terms.is_empty()
                        {
                            for range in crate::domain::query::highlight_ranges(&text, terms) {
                                for part in layout.range_rects(range.start, range.end) {
                                    canvas.fill(part.offset(origin.x, origin.y), SEARCH_HIGHLIGHT);
                                }
                            }
                        }
                        canvas.draw_layout(&layout, origin, foreground);
                        canvas.pop_clip();
                        if active_cell {
                            canvas.stroke(cell, active_border, 2.0);
                        }
                        if hidden || layout.is_trimmed() {
                            self.overflowing.insert((row, column));
                        }
                        canvas.hit(cell, Hit::Grid(GridHit::Cell { row, column }));
                    }
                    if !pass_frozen {
                        canvas.pop_clip();
                    }
                }
            }
            canvas.pop_clip();
        }
        // Footer.
        if let Some(footer) = self.footer.clone() {
            let footer_row = rect(bounds.x, rows_area.bottom() + if horizontal { bar } else { 0.0 }, bounds.w, row_height);
            canvas.fill(footer_row, head);
            canvas.fill(rect(footer_row.x, footer_row.y, footer_row.w, 1.0), grid_border);
            let bold = font.semibold();
            for pass_frozen in [false, true] {
                if !pass_frozen {
                    canvas.push_clip(rect(scroll_clip.x, footer_row.y, scroll_clip.w, footer_row.h));
                }
                for &column in &visible {
                    let is_frozen = column < frozen_count;
                    if is_frozen != pass_frozen {
                        continue;
                    }
                    let Some(x) = geometry.column_x(column) else {
                        continue;
                    };
                    let cell = rect(column_left(column, x), footer_row.y, self.widths[column], row_height);
                    let value = footer.value(&self.result.columns[column].0);
                    let text = match value {
                        Some(value) => cell_preview(value.as_deref(), &self.result.dtypes[column], self.formats[column]).0,
                        None if column == 0 => footer.label.clone(),
                        None => String::new(),
                    };
                    if is_frozen {
                        canvas.fill(cell, frozen_head);
                    }
                    canvas.fill(rect(cell.right() - 1.0, cell.y, 1.0, cell.h), grid_border);
                    let align = if value.is_some() && self.formats[column].is_some() {
                        Align::Right
                    } else {
                        Align::Left
                    };
                    canvas.text(rect(cell.x + 8.0, cell.y, (cell.w - 17.0).max(1.0), cell.h), &text, &bold, foreground, align);
                }
                if !pass_frozen {
                    canvas.pop_clip();
                }
            }
        }
        // Scroll bars.
        if row_count > 0 {
            let vertical = rect(bounds.right() - bar, rows_area.y, bar, rows_area.h);
            self.layout.vertical_bar = vertical;
            scroll::vertical(canvas, vertical, rows_area.h, content_height, &self.v_scroll, Hit::Grid(GridHit::VerticalBar));
        }
        if horizontal {
            let track = rect(bounds.x + frozen_width, rows_area.bottom(), geometry.scrollable_width(), bar);
            self.layout.horizontal_bar = track;
            self.h_scroll.offset = self.scroll_x;
            scroll::horizontal(
                canvas,
                track,
                geometry.scrollable_width(),
                geometry.content_width(),
                &self.h_scroll,
                Hit::Grid(GridHit::HorizontalBar),
            );
        }
    }

    /// Draws the open cell tooltip; call after everything it may cover.
    pub fn paint_tooltip(&mut self, canvas: &mut Canvas<Hit>, font: &Font) {
        let Some(tooltip) = &mut self.tooltip else {
            return;
        };
        let Some(x) = GridGeometry::new(self.widths.clone(), self.frozen_count, self.layout.rows.w, self.scroll_x)
            .column_x(tooltip.column)
        else {
            return;
        };
        let cell_y = self.layout.rows.y + tooltip.row as f32 * self.row_height - self.v_scroll.offset;
        let width = (canvas.width - 64.0).clamp(1.0, 560.0);
        let max_height = (canvas.height - 64.0).clamp(1.0, 360.0);
        let layout = text::layout(&tooltip.text, font, TextOptions::wrapped(width - 16.0 - scroll::BAR_WIDTH));
        tooltip.content_height = layout.height;
        let height = (layout.height + 16.0).min(max_height);
        let mut left = (self.layout.bounds.x + x).min(canvas.width - width - 8.0).max(8.0);
        let mut top = cell_y + self.row_height + 2.0;
        if top + height > canvas.height - 8.0 {
            top = (cell_y - height - 2.0).max(8.0);
        }
        if left < 0.0 {
            left = 8.0;
        }
        let r = rect(left, top, width, height);
        tooltip.rect = r;
        tooltip.scroll.clamp(height - 16.0, layout.height);
        canvas.shadow(r, 6.0);
        canvas.fill_rounded(r, 6.0, colors::POPOVER);
        canvas.stroke_rounded(r, 6.0, colors::BORDER, 1.0);
        canvas.push_clip(r.inset(8.0));
        canvas.draw_layout(&layout, point(r.x + 8.0, r.y + 8.0 - tooltip.scroll.offset), colors::FOREGROUND);
        canvas.pop_clip();
        canvas.hit(r, Hit::Grid(GridHit::Tooltip));
        let bar = rect(r.right() - scroll::BAR_WIDTH, r.y, scroll::BAR_WIDTH, r.h);
        scroll::vertical(canvas, bar, height - 16.0, layout.height, &tooltip.scroll, Hit::Grid(GridHit::TooltipBar));
    }

    /// The rows' visible top, for tests.
    #[cfg(test)]
    pub(crate) fn scroll_offset(&self) -> f32 {
        self.v_scroll.offset
    }

    #[cfg(test)]
    pub(crate) fn max_scroll(&self) -> f32 {
        scroll::Scroll::max(self.layout.rows.h, self.result.row_count() as f32 * self.row_height)
    }
}

#[cfg(test)]
mod tests {
    use super::{GridHost, GridView, TOOLTIP_DELAY, WidthCandidates};
    use crate::domain::frame::{FrameSnapshot, ResultSnapshot};
    use crate::domain::selection::Cell;
    use crate::infrastructure::duckdb_store::DuckDbStore;
    use crate::ui::hit::{GridHit, Hit};
    use crate::ui::platform::headless::TestApp;
    use crate::ui::platform::{Canvas, Cx, Event, Font, Point, Rect, View, WindowId, point, rect};
    use duckdb::types::Value;

    /// A window holding only a grid, `inset` pixels below the top.
    struct Host {
        grid: GridView,
        inset: f32,
        font: Font,
    }

    impl GridHost for Host {
        fn grid_mut(&mut self) -> &mut GridView {
            &mut self.grid
        }
    }

    impl View for Host {
        type Hit = Hit;

        fn paint(&mut self, canvas: &mut Canvas<Hit>, cx: &mut Cx<Self>) {
            let bounds = rect(0.0, self.inset, canvas.width, canvas.height - self.inset);
            let font = self.font.clone();
            self.grid.paint(canvas, bounds, &font, cx);
            self.grid.paint_tooltip(canvas, &font);
        }

        fn event(&mut self, event: &Event<Hit>, cx: &mut Cx<Self>) -> bool {
            let grid_hit = |hit: &Option<Hit>| match hit {
                Some(Hit::Grid(hit)) => Some(*hit),
                _ => None,
            };
            let (_, height) = cx.viewport();
            match event {
                Event::Key(key) => self.grid.key(key, height),
                Event::MouseDown { pos, modifiers, hit, .. } => match grid_hit(hit) {
                    Some(hit) => self.grid.mouse_down(hit, *pos, modifiers.shift, false),
                    None => false,
                },
                Event::MouseMove { pos, left_down, hit, .. } => {
                    if *left_down {
                        self.grid.mouse_drag(*pos);
                    }
                    self.grid.mouse_move(grid_hit(hit), cx);
                    true
                }
                Event::MouseUp { .. } => {
                    self.grid.mouse_up();
                    true
                }
                Event::Wheel { delta, modifiers, hit, .. } => self.grid.wheel(grid_hit(hit), *delta, modifiers.shift),
                _ => false,
            }
        }
    }

    fn source(columns: &[(&str, &str)], rows: Vec<Vec<Value>>) -> FrameSnapshot {
        let columns = columns
            .iter()
            .map(|(name, kind)| ((*name).to_owned(), (*kind).to_owned()))
            .collect::<Vec<_>>();
        FrameSnapshot::new(DuckDbStore::from_rows(&columns, rows).unwrap(), 1)
    }

    fn open(app: &TestApp<Host>, width: f32, height: f32, inset: f32, result: ResultSnapshot) -> WindowId {
        result.prefetch_page(0).unwrap();
        let id = app.open(width, height, move |_| Host {
            grid: GridView::new(result),
            inset,
            font: Font::new(crate::ui::widgets::DEFAULT_FAMILY, 14.0),
        });
        app.render(id);
        id
    }

    fn text_grid(app: &TestApp<Host>, values: Vec<Value>) -> WindowId {
        let source = source(&[("Text", "VARCHAR")], values.into_iter().map(|value| vec![value]).collect());
        let id = open(app, 500.0, 350.0, 0.0, ResultSnapshot::from_source(&source, 1, 1));
        app.update(id, |host, _| {
            host.grid.widths[0] = 100.0;
            host.grid.set_frozen_count(1);
        });
        app.render(id);
        id
    }

    /// Where the cell is after painting the current state.
    fn cell(app: &TestApp<Host>, id: WindowId, row: usize, column: usize) -> Rect {
        app.render(id);
        app.find(id, |hit| *hit == Hit::Grid(GridHit::Cell { row, column }))
            .expect("the cell is painted")
    }

    fn header(app: &TestApp<Host>, id: WindowId, column: usize) -> Rect {
        app.find(id, |hit| *hit == Hit::Grid(GridHit::Header(column)))
            .expect("the header is painted")
    }

    fn hover(app: &TestApp<Host>, id: WindowId, pos: Point) {
        app.render(id);
        app.mouse_move(id, pos, false);
        app.render(id);
    }

    /// Waits out the tooltip delay, then returns the tooltip's text.
    fn tooltip_after_delay(app: &TestApp<Host>, id: WindowId) -> Option<String> {
        app.advance_clock(TOOLTIP_DELAY);
        app.render(id);
        app.read(id, |host| host.grid.tooltip.as_ref().map(|tooltip| tooltip.text.clone()))
    }

    #[test]
    fn long_cells_stay_on_one_line_and_hover_reveals_scrollable_full_text() {
        let long = format!("{}THE END", "Long Unicode text \u{754c}\u{1f600}\twith details\n".repeat(60));
        let app = TestApp::new();
        let id = text_grid(&app, vec![Value::Text("short".into()), Value::Text(long.clone()), Value::Null]);
        let first = cell(&app, id, 0, 0);
        let second = cell(&app, id, 1, 0);
        assert_eq!(first.h, second.h);
        assert_eq!(second.y, first.bottom());
        let (preview, hidden) = super::cell_preview(Some(&long), "VARCHAR", None);
        assert!(!preview.contains(['\n', '\t']));
        assert!(preview.ends_with('\u{2026}'));
        assert!(hidden);

        hover(&app, id, second.center());
        assert_eq!(tooltip_after_delay(&app, id).as_deref(), Some(long.as_str()));
        let scroll = app
            .find(id, |hit| *hit == Hit::Grid(GridHit::Tooltip))
            .expect("the tooltip is painted");
        assert!(scroll.w <= 500.0 && scroll.h <= 350.0);
        app.scroll(id, scroll.center(), point(0.0, -10.0));
        app.render(id);
        assert!(app.read(id, |host| host.grid.tooltip.as_ref().unwrap().scroll.offset) > 0.0);
        // Moving onto the tooltip keeps it open.
        app.mouse_move(id, scroll.center(), false);
        assert!(app.read(id, |host| host.grid.tooltip.is_some()));

        hover(&app, id, point(495.0, 345.0));
        assert!(app.read(id, |host| host.grid.tooltip.is_none()));
        // Short and empty cells have nothing more to show.
        hover(&app, id, cell(&app, id, 0, 0).center());
        assert_eq!(tooltip_after_delay(&app, id), None);
        hover(&app, id, cell(&app, id, 2, 0).center());
        assert_eq!(tooltip_after_delay(&app, id), None);
    }

    #[test]
    fn hover_tracks_replaced_results_and_current_width() {
        let app = TestApp::new();
        let id = text_grid(
            &app,
            vec![Value::Text("WWWWWWWWWWWWWWWWWWWW".into()), Value::Text("another value".into())],
        );
        app.update(id, |host, _| host.grid.widths[0] = 40.0);
        hover(&app, id, cell(&app, id, 0, 0).center());
        assert_eq!(tooltip_after_delay(&app, id).as_deref(), Some("WWWWWWWWWWWWWWWWWWWW"));

        let replacement = source(&[("Text", "VARCHAR")], vec![vec![Value::Text("Replacement value".into())]]);
        let result = ResultSnapshot::from_source(&replacement, 2, 2);
        result.prefetch_page(0).unwrap();
        app.update(id, |host, _| host.grid.replace_result(result));
        app.render(id);
        // The old tooltip belonged to the old result.
        app.update(id, |host, _| host.grid.mouse_leave());
        hover(&app, id, cell(&app, id, 0, 0).center());
        assert_eq!(tooltip_after_delay(&app, id).as_deref(), Some("Replacement value"));

        app.update(id, |host, _| {
            host.grid.widths[0] = 400.0;
            host.grid.mouse_leave();
        });
        hover(&app, id, cell(&app, id, 0, 0).center());
        assert_eq!(tooltip_after_delay(&app, id), None);
    }

    #[test]
    fn hover_resolves_sorted_rows_and_reordered_columns() {
        use crate::domain::documents::SortKey;
        use crate::infrastructure::duckdb_store::ViewQuery;

        let original = "original row ".repeat(30);
        let sorted = "sorted row ".repeat(30);
        let source = source(
            &[("Key", "VARCHAR"), ("Text", "VARCHAR"), ("Amount", "DOUBLE")],
            vec![
                vec![Value::Text("z".into()), Value::Text(original.clone()), Value::Double(1.0)],
                vec![Value::Text("a".into()), Value::Text(sorted.clone()), Value::Double(987654321.123)],
            ],
        );
        let app = TestApp::new();
        let id = open(&app, 500.0, 350.0, 0.0, ResultSnapshot::from_source(&source, 1, 1));
        hover(&app, id, cell(&app, id, 0, 1).center());
        assert_eq!(tooltip_after_delay(&app, id).as_deref(), Some(original.as_str()));

        let (rows, schema) = {
            let store = source.store.lock().unwrap();
            let rows = store
                .query_view_rows(&ViewQuery {
                    hidden: Vec::new(),
                    value_filters: Vec::new(),
                    tokens: Vec::new(),
                    filters: Vec::new(),
                    sort: vec![SortKey {
                        column: "Key".into(),
                        descending: false,
                    }],
                })
                .unwrap();
            (rows, store.schema().to_vec())
        };
        let result = ResultSnapshot::from_view(&source, rows, schema, 2, 2).reordered(
            &["Amount".into(), "Text".into(), "Key".into()],
            3,
            3,
        );
        result.prefetch_page(0).unwrap();
        app.update(id, |host, _| {
            host.grid.replace_result(result);
            host.grid.widths[0] = 40.0;
            host.grid.set_frozen_count(1);
            host.grid.mouse_leave();
        });
        hover(&app, id, cell(&app, id, 0, 1).center());
        assert_eq!(tooltip_after_delay(&app, id).as_deref(), Some(sorted.as_str()));
        hover(&app, id, point(495.0, 345.0));
        assert!(app.read(id, |host| host.grid.tooltip.is_none()));
        hover(&app, id, cell(&app, id, 0, 0).center());
        assert_eq!(tooltip_after_delay(&app, id).as_deref(), Some("987,654,321.12"));
    }

    #[test]
    fn inference_measures_four_columns_per_frame_and_reset_rejects_old_candidates() {
        let columns = (0..6).map(|index| format!("c{index}")).collect::<Vec<_>>();
        let source = source(
            &columns.iter().map(|name| (name.as_str(), "VARCHAR")).collect::<Vec<_>>(),
            vec![vec![Value::Text("\u{754c} WWW".into()); 6]],
        );
        let app = TestApp::new();
        let id = open(&app, 800.0, 400.0, 0.0, ResultSnapshot::from_source(&source, 1, 1));
        app.update(id, |host, cx| host.grid.configure_widths("source".into(), Some(source), Default::default(), cx));
        app.run_until_parked();
        app.update(id, |host, _| {
            let grid = &mut host.grid;
            grid.reset_widths();
            for column in &columns {
                grid.pending_widths.insert(column.clone());
                grid.width_candidates.push_back(WidthCandidates {
                    column: column.clone(),
                    samples: vec!["WWW".into()],
                    epoch: grid.width_epoch,
                    revision: 0,
                });
            }
        });
        app.render(id);
        let counts = |app: &TestApp<Host>| {
            app.read(id, |host| (host.grid.known_widths.len(), host.grid.pending_widths.len()))
        };
        assert_eq!(counts(&app), (4, 2));
        // The next frame measures the rest.
        app.render(id);
        assert_eq!(counts(&app), (6, 0));
        assert!(app.read(id, |host| host.grid.widths.iter().all(|width| (80.0..=600.0).contains(width))));

        app.update(id, |host, _| {
            let grid = &mut host.grid;
            let old_epoch = grid.width_epoch;
            grid.reset_widths();
            grid.width_candidates.push_back(WidthCandidates {
                column: "c0".into(),
                samples: vec!["W".repeat(200)],
                epoch: old_epoch,
                revision: 0,
            });
        });
        app.render(id);
        assert_eq!(app.read(id, |host| host.grid.column_width("c0")), 200.0);
        app.update(id, |host, _| {
            host.grid.select(Cell { row: 0, column: 0 }, false);
            host.grid.widths[0] = 1195.0;
        });
        app.press(id, ")");
        assert_eq!(app.read(id, |host| host.grid.column_width("c0")), 1200.0);
        app.update(id, |host, _| host.grid.widths[0] = 45.0);
        app.press(id, "(");
        assert_eq!(app.read(id, |host| host.grid.column_width("c0")), 40.0);
    }

    #[test]
    fn vertical_thumb_tracks_drag_wheel_and_keyboard_in_an_inset_grid() {
        let source = source(&[("ID", "BIGINT")], (0..1000).map(|row| vec![Value::BigInt(row)]).collect());
        let app = TestApp::new();
        let id = open(&app, 500.0, 350.0, 70.0, ResultSnapshot::from_source(&source, 1, 1));
        let offset = |app: &TestApp<Host>| app.read(id, |host| host.grid.scroll_offset());
        let max = app.read(id, |host| host.grid.max_scroll());
        let bar = app
            .find(id, |hit| *hit == Hit::Grid(GridHit::VerticalBar))
            .expect("the scroll bar is painted");
        assert!(bar.y >= 118.0);
        assert!(bar.h < 250.0);
        let top = point(bar.center().x, bar.y + 16.0);
        let middle = point(top.x, bar.center().y);
        app.drag(id, top, middle);
        let dragged = offset(&app);
        assert!(dragged > 1000.0, "drag must scroll the rows");

        // Grabbing the moved thumb again must not jump to another position.
        app.drag(id, middle, middle);
        assert!((offset(&app) - dragged).abs() < 1.0);
        app.drag(id, middle, top);
        assert!(offset(&app).abs() < 1.0);

        app.scroll(id, cell(&app, id, 0, 0).center(), point(0.0, -400.0));
        app.render(id);
        let wheel_offset = offset(&app);
        assert!(wheel_offset > 0.0);
        let thumb = point(top.x, bar.y + 12.0 + (bar.h - 24.0) * (wheel_offset / max));
        app.drag(id, thumb, thumb);
        assert!((offset(&app) - wheel_offset).abs() < 1.0);

        app.update(id, |host, _| host.grid.select(Cell { row: 0, column: 0 }, false));
        app.press(id, "ctrl-end");
        app.render(id);
        assert_eq!(app.read(id, |host| host.grid.selection().unwrap().active.row), 999);
        assert!((offset(&app) - max).abs() < 1.0);
        let bottom = point(top.x, bar.bottom() - 16.0);
        app.drag(id, bottom, bottom);
        assert!((offset(&app) - max).abs() < 1.0);
        app.drag(id, bottom, point(top.x, bar.y));
        assert!(offset(&app).abs() < 1.0);
    }

    #[test]
    fn dragging_across_cells_selects_a_rectangle_without_scrolling_the_view() {
        let source = source(
            &[("A", "BIGINT"), ("B", "BIGINT"), ("C", "BIGINT")],
            (0..100).map(|row| vec![Value::BigInt(row); 3]).collect(),
        );
        let app = TestApp::new();
        let id = open(&app, 500.0, 350.0, 0.0, ResultSnapshot::from_source(&source, 1, 1));
        app.drag(id, cell(&app, id, 0, 0).center(), cell(&app, id, 4, 2).center());
        let selection = app.read(id, |host| host.grid.selection().unwrap());
        assert_eq!(selection.anchor, Cell { row: 0, column: 0 });
        assert_eq!(selection.active, Cell { row: 4, column: 2 });
        assert_eq!(app.read(id, |host| host.grid.scroll_offset()), 0.0);
    }

    #[test]
    fn frozen_pane_stays_unchanged_when_scrolling_and_header_wraps() {
        let text = |values: [&str; 4]| values.map(|value| Value::Text(value.into())).to_vec();
        let source = source(
            &[
                ("ID", "VARCHAR"),
                ("a_long_column_name_that_needs_several_lines_in_the_header", "VARCHAR"),
                ("third", "VARCHAR"),
                ("fourth", "VARCHAR"),
            ],
            vec![
                text(["one", "scrolling content one", "three", "five"]),
                text(["two", "scrolling content two", "four", "six"]),
            ],
        );
        let app = TestApp::new();
        let id = open(&app, 500.0, 350.0, 0.0, ResultSnapshot::from_source(&source, 1, 1));
        app.update(id, |host, _| host.grid.set_frozen_count(1));
        app.render(id);
        let frozen_header = header(&app, id, 0);
        let scrolling_header = header(&app, id, 1);
        let frozen_cell = cell(&app, id, 0, 0);
        let scrolling_cell = cell(&app, id, 0, 1);
        assert!(frozen_header.h > 48.0);
        assert_eq!(scrolling_header.x, frozen_header.right());
        assert_eq!(scrolling_cell.x, frozen_cell.right());
        assert!(frozen_cell.y >= frozen_header.bottom());

        app.update(id, |host, _| host.grid.scroll_horizontal(120.0, 500.0));
        app.render(id);
        assert_eq!(app.read(id, |host| host.grid.scroll_x), 120.0);
        // Regions are clipped to the scrolling pane, so compare right edges.
        assert_eq!(header(&app, id, 1).right(), scrolling_header.right() - 120.0);
        assert_eq!(header(&app, id, 0), frozen_header);
        assert_eq!(cell(&app, id, 0, 0), frozen_cell);
        assert_eq!(cell(&app, id, 0, 1).right(), scrolling_cell.right() - 120.0);
    }
}

#[cfg(test)]
mod preview_tests {
    use super::{CELL_PREVIEW_LIMIT, cell_preview};
    use crate::domain::formatting::NumericFormat;

    #[test]
    fn bounded_preview_preserves_unicode_and_marks_hidden_whitespace() {
        let (text, hidden) = cell_preview(Some(&"界😀".repeat(101)), "VARCHAR", None);
        assert_eq!(text.chars().count(), CELL_PREVIEW_LIMIT + 1);
        assert!(text.ends_with('…'));
        assert!(hidden);
        assert_eq!(
            cell_preview(Some("one\r\ntwo\tthree"), "VARCHAR", None),
            ("one two three".into(), true)
        );
        assert_eq!(
            cell_preview(Some("short"), "VARCHAR", None),
            ("short".into(), false)
        );
        assert_eq!(cell_preview(None, "VARCHAR", None), (String::new(), false));
        assert_eq!(
            cell_preview(Some(""), "VARCHAR", None),
            (String::new(), false)
        );
        assert_eq!(
            cell_preview(
                Some("18446744073709551615"),
                "UBIGINT",
                Some(NumericFormat {
                    precision: 0,
                    grouping: true
                })
            ),
            ("18,446,744,073,709,551,615".into(), false)
        );
    }
}
