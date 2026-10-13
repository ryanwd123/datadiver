#[cfg(feature = "snapshots")]
use gpui_kit::TestSupportExt;
use gpui_kit::component::scroll::{ScrollableElement, Scrollbar, ScrollbarMode};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::rc::Rc;

const SCROLLBAR_WIDTH: f32 = 12.0;
const SCROLLBAR_THUMB: f32 = 6.0;
const MIN_THUMB_LEN: f32 = 24.0;
const CELL_PREVIEW_LIMIT: usize = 200;
const SEARCH_HIGHLIGHT: u32 = 0xffd84d99;

fn cell_preview(value: Option<&str>, dtype: &str, format: Option<NumericFormat>) -> (String, bool) {
    let text = display_cell(value, dtype, format, CELL_PREVIEW_LIMIT);
    let hidden = text.chars().count() > CELL_PREVIEW_LIMIT || text.contains(['\r', '\n', '\t']);
    (
        text.replace("\r\n", " ").replace(['\r', '\n', '\t'], " "),
        hidden,
    )
}

fn cell_tooltip(text: String, window: &mut Window, cx: &mut App) -> AnyView {
    let width = (window.viewport_size().width.as_f32() - 64.0).clamp(1.0, 560.0);
    let height = (window.viewport_size().height.as_f32() - 64.0).clamp(1.0, 360.0);
    let text: SharedString = text.into();
    let scroll_handle = ScrollHandle::new();
    Tooltip::element(move |_, _| {
        let value = div()
            .id("grid-cell-tooltip-value")
            .aria_label(text.clone())
            .whitespace_normal()
            .child(text.clone());
        #[cfg(feature = "snapshots")]
        let value = value.test_support();
        let scroll = div()
            .id("grid-cell-tooltip-scroll")
            .w(px(width))
            .max_h(px(height))
            .overflow_y_scroll()
            .track_scroll(&scroll_handle)
            .vertical_scrollbar(&scroll_handle)
            .pr(px(SCROLLBAR_WIDTH))
            .child(value);
        #[cfg(feature = "snapshots")]
        let scroll = scroll.test_support();
        scroll
    })
    .build(window, cx)
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

pub struct GridView {
    result: ResultSnapshot,
    theme: ThemeSettings,
    keymap: KeymapFile,
    key_context: KeyContext,
    selection: Option<Selection>,
    widths: Vec<f32>,
    width_owner: String,
    width_store: usize,
    width_epoch: u64,
    known_widths: BTreeMap<String, f32>,
    width_revisions: HashMap<String, u64>,
    pending_widths: HashSet<String>,
    width_candidates: VecDeque<WidthCandidates>,
    width_batch_blocked: bool,
    row_height: f32,
    typography: Option<(Font, Pixels)>,
    formats: Vec<Option<NumericFormat>>,
    frozen_count: usize,
    scroll_x: f32,
    h_scroll_drag: Option<f32>,
    selection_drag: Option<Cell>,
    sort: Vec<SortKey>,
    focus: FocusHandle,
    vertical_scroll: UniformListScrollHandle,
    pending_pages: HashSet<(u64, usize)>,
    header_cache: Option<HeaderCache>,
    benchmark_rendered_revision: Option<u64>,
    highlights: crate::domain::query::SearchHighlights,
    footer: Option<GridFooter>,
    /// Widen columns to fit the footer once width inference settles.
    footer_fit: bool,
    /// Heat-map ranges by column name; cells of other columns are not shaded.
    heat_ranges: std::collections::HashMap<String, (f64, f64)>,
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
    font_size: Pixels,
    line_height: Pixels,
    rem_size: Pixels,
    labels: Vec<String>,
    height: f32,
}

struct WidthCandidates {
    column: String,
    samples: Vec<String>,
    epoch: u64,
    revision: u64,
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
    },
    SelectionCleared,
    MoveColumn(isize),
    ToggleFrozen,
}

impl EventEmitter<GridEvent> for GridView {}

/// A heat-map cell background: a translucent green, stronger for larger
/// values, so it reads on light and dark themes.
fn heat_color(fraction: f32) -> Hsla {
    hsla(0.36, 0.6, 0.45, 0.06 + 0.5 * fraction)
}

impl GridView {
    #[cfg(all(test, feature = "snapshots"))]
    pub(crate) fn numeric_format(&self, name: &str) -> Option<NumericFormat> {
        self.result
            .columns
            .iter()
            .position(|column| column.0 == name)
            .and_then(|index| self.formats[index])
    }
    pub fn configure_formats(
        &mut self,
        saved: &BTreeMap<String, NumericFormat>,
        cx: &mut Context<Self>,
    ) {
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
        cx.notify();
    }

    pub fn change_numeric_format(&mut self, action: ActionId, cx: &mut Context<Self>) {
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
            cx.emit(GridEvent::NumericFormatChanged {
                column: self.result.columns[column].0.clone(),
                format,
            });
            cx.notify();
        }
    }

    pub(crate) fn benchmark_scroll_to(&mut self, row: usize, cx: &mut Context<Self>) {
        self.vertical_scroll
            .scroll_to_item(row, ScrollStrategy::Top);
        cx.notify();
    }
    pub fn new(result: ResultSnapshot, cx: &mut Context<Self>) -> Self {
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
            h_scroll_drag: None,
            selection_drag: None,
            sort: Vec::new(),
            focus: cx.focus_handle(),
            vertical_scroll: UniformListScrollHandle::new(),
            pending_pages: HashSet::new(),
            header_cache: None,
            benchmark_rendered_revision: None,
            highlights: Default::default(),
            footer: None,
            footer_fit: false,
            heat_ranges: Default::default(),
        }
    }

    pub fn set_heat_ranges(
        &mut self,
        ranges: std::collections::HashMap<String, (f64, f64)>,
        cx: &mut Context<Self>,
    ) {
        if self.heat_ranges != ranges {
            self.heat_ranges = ranges;
            cx.notify();
        }
    }

    pub fn set_footer(&mut self, footer: Option<GridFooter>, cx: &mut Context<Self>) {
        if self.footer != footer {
            self.footer_fit = footer.is_some();
            self.footer = footer;
            cx.notify();
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

    #[cfg(all(test, feature = "snapshots"))]
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

    #[cfg(all(test, feature = "snapshots"))]
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

    pub fn configure_widths(
        &mut self,
        owner: String,
        source: Option<FrameSnapshot>,
        saved: BTreeMap<String, f32>,
        cx: &mut Context<Self>,
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
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    store
                        .lock()
                        .map_err(|_| "Width sampling store lock poisoned".to_owned())?
                        .width_candidates(rows.as_ref(), &columns)
                })
                .await;
            let _ = this.update(cx, |grid, cx| {
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
                        cx.emit(GridEvent::WidthInferenceFailed {
                            owner: grid.width_owner.clone(),
                            error,
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn change_width(&mut self, increase: bool, window: &Window, cx: &mut Context<Self>) {
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
        self.keep_active_column_visible(self.viewport_width(window));
        cx.emit(GridEvent::WidthChanged {
            owner: self.width_owner.clone(),
            column: name,
            width,
        });
        cx.notify();
    }

    fn viewport_width(&self, window: &Window) -> f32 {
        let width = self
            .vertical_scroll
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .width
            .as_f32();
        if width > 0.0 {
            width
        } else {
            window.bounds().size.width.as_f32().max(0.0)
        }
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

    pub fn set_theme(&mut self, theme: ThemeSettings, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }
    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    pub fn set_keymap(&mut self, keymap: KeymapFile, context: KeyContext) {
        self.keymap = keymap;
        self.key_context = context;
    }

    pub fn set_sort(&mut self, sort: Vec<SortKey>, cx: &mut Context<Self>) {
        self.sort = sort;
        self.header_cache = None;
        cx.notify();
    }

    /// Search words to highlight in cells.
    pub fn set_highlights(
        &mut self,
        highlights: crate::domain::query::SearchHighlights,
        cx: &mut Context<Self>,
    ) {
        if self.highlights != highlights {
            self.highlights = highlights;
            cx.notify();
        }
    }

    /// Moves the active cell to `column`, keeping its row, and scrolls it into view.
    pub fn jump_to_column(&mut self, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        if column >= self.result.column_count() || self.result.row_count() == 0 {
            return;
        }
        let row = self
            .selection
            .map_or(0, |selection| selection.active.row)
            .min(self.result.row_count() - 1);
        self.select(Cell { row, column }, false, cx);
        self.vertical_scroll
            .scroll_to_item(row, ScrollStrategy::Nearest);
        self.keep_active_column_visible(self.viewport_width(window));
        cx.notify();
    }

    pub fn frozen_count(&self) -> usize {
        self.frozen_count
    }

    pub fn set_frozen_count(&mut self, count: usize, cx: &mut Context<Self>) {
        self.frozen_count = count.min(self.result.column_count());
        cx.notify();
    }

    pub fn replace_result(&mut self, result: ResultSnapshot, cx: &mut Context<Self>) {
        let old_widths = self
            .result
            .columns
            .iter()
            .zip(&self.widths)
            .map(|(column, width)| (column.0.as_str(), *width))
            .collect::<std::collections::HashMap<_, _>>();
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
        self.vertical_scroll.scroll_to_item(0, ScrollStrategy::Top);
        if let Some(selection) = self.selection {
            cx.emit(GridEvent::SelectionChanged {
                result: self.result.clone(),
                selection,
            });
        } else {
            cx.emit(GridEvent::SelectionCleared);
        }
        cx.notify();
    }

    pub fn select(&mut self, cell: Cell, extend: bool, cx: &mut Context<Self>) {
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
        cx.emit(GridEvent::SelectionChanged {
            result: self.result.clone(),
            selection: self.selection.unwrap(),
        });
        cx.notify();
    }

    pub fn reset_navigation_position(&mut self, cx: &mut Context<Self>) {
        self.selection = None;
        self.scroll_x = 0.0;
        self.vertical_scroll.scroll_to_item(0, ScrollStrategy::Top);
        if self.result.row_count() > 0 && self.result.column_count() > 0 {
            self.select(Cell { row: 0, column: 0 }, false, cx);
        } else {
            cx.emit(GridEvent::SelectionCleared);
            cx.notify();
        }
    }

    fn begin_selection_drag(&mut self, cell: Cell, extend: bool, cx: &mut Context<Self>) {
        self.selection_drag = Some(if extend {
            self.selection
                .map(|selection| selection.anchor)
                .unwrap_or(cell)
        } else {
            cell
        });
        self.select(cell, extend, cx);
    }

    fn extend_selection_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(anchor) = self.selection_drag else {
            return;
        };
        let scroll = self.vertical_scroll.0.borrow().base_handle.clone();
        let bounds = scroll.bounds();
        let width = bounds.size.width.as_f32();
        let height = bounds.size.height.as_f32();
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let x = (f32::from(position.x) - f32::from(bounds.left())).clamp(0.0, width - 0.01);
        let y = (f32::from(position.y) - f32::from(bounds.top())).clamp(0.0, height - 0.01);
        let geometry =
            GridGeometry::new(self.widths.clone(), self.frozen_count, width, self.scroll_x);
        let Some(column) = geometry.hit_test_column(x) else {
            return;
        };
        let offset_y = f32::from(scroll.offset().y);
        let row = ((y - offset_y) / self.row_height).floor().max(0.0) as usize;
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
        cx.emit(GridEvent::SelectionChanged {
            result: self.result.clone(),
            selection: self.selection.unwrap(),
        });
        cx.notify();
    }

    pub fn scroll_horizontal(&mut self, delta: f32, viewport_width: f32, cx: &mut Context<Self>) {
        let geometry = GridGeometry::new(
            self.widths.clone(),
            self.frozen_count,
            viewport_width,
            self.scroll_x,
        );
        self.scroll_x = (self.scroll_x + delta).clamp(0.0, geometry.max_scroll_x());
        cx.notify();
    }

    pub fn navigate_action(
        &mut self,
        action: ActionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let page = ((window.bounds().size.height.as_f32() - 120.0) / self.row_height)
            .floor()
            .max(1.0) as usize;
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
        cx.emit(GridEvent::SelectionChanged {
            result: self.result.clone(),
            selection: self.selection.unwrap(),
        });
        self.vertical_scroll
            .scroll_to_item(next.row, ScrollStrategy::Nearest);
        self.keep_active_column_visible(self.viewport_width(window));
        cx.notify();
    }

    fn prefetch_page(&mut self, page_index: usize, cx: &mut Context<Self>) {
        let key = (self.result.result_revision, page_index);
        if self.result.page_is_cached(page_index) || !self.pending_pages.insert(key) {
            return;
        }
        let result = self.result.clone();
        cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_executor()
                .spawn(async move { result.prefetch_page(page_index) })
                .await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |grid, cx| {
                    // A failed page stays pending until the result is replaced; retrying
                    // on every render would loop the same failing query.
                    if fetched.is_err() {
                        return;
                    }
                    grid.pending_pages.remove(&key);
                    if grid.result.result_revision == key.0 {
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
}

impl Focusable for GridView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn wrapped_line_count(window: &Window, text: &str, width: f32, style: &TextStyle) -> usize {
    let font_size = style.font_size.to_pixels(window.rem_size());
    window
        .text_system()
        .shape_text(
            text.to_owned().into(),
            font_size,
            &[style.to_run(text.len())],
            Some(px(width.max(1.0))),
            None,
        )
        .map(|lines| {
            lines
                .iter()
                .map(|line| line.wrap_boundaries().len() + 1)
                .sum::<usize>()
                .max(1)
        })
        .unwrap_or_else(|_| text.lines().count().max(1))
}

impl Render for GridView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _render = crate::infrastructure::performance::Span::new("grid_render");
        if self.result.row_count() == 0 {
            self.benchmark_rendered_revision = Some(self.result.result_revision);
        }
        let viewport_width = self.viewport_width(window);
        let text_style = window.text_style();
        let rem_size = window.rem_size();
        let font = text_style.font();
        let font_size = text_style.font_size.to_pixels(rem_size);
        let line_height = text_style.line_height_in_pixels(rem_size);
        let row_height = (line_height.as_f32() + 6.0).ceil().max(26.0);
        if self
            .typography
            .as_ref()
            .is_some_and(|previous| previous != &(font.clone(), font_size))
        {
            let scroll = self.vertical_scroll.0.borrow().base_handle.clone();
            let offset = scroll.offset();
            scroll.set_offset(point(offset.x, offset.y * (row_height / self.row_height)));
            if let Some(selection) = self.selection {
                self.vertical_scroll
                    .scroll_to_item(selection.active.row, ScrollStrategy::Nearest);
            }
            self.header_cache = None;
        }
        let mut inferred = false;
        let mut processed_candidates = false;
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
            let measure = |text: &str, style: &TextStyle| {
                let size = style.font_size.to_pixels(rem_size);
                let width = window
                    .text_system()
                    .shape_line(
                        text.to_owned().into(),
                        size,
                        &[style.to_run(text.len())],
                        None,
                    )
                    .width()
                    .as_f32();
                if width.is_finite() { width } else { f32::NAN }
            };
            let mut dtype_style = text_style.clone();
            dtype_style.font_size = rems(0.75).into();
            let name_width = measure(&candidate.column, &text_style);
            let dtype_width = measure(&type_label(&self.result.dtypes[column]), &dtype_style);
            let mut measured = if name_width.is_finite() && dtype_width.is_finite() {
                name_width.max(dtype_width)
            } else {
                f32::NAN
            };
            for text in &candidate.samples {
                if !measured.is_finite() {
                    break;
                }
                let width = measure(text, &text_style);
                if !width.is_finite() {
                    measured = f32::NAN;
                    break;
                }
                measured = measured.max(width);
            }
            let width = inferred_width(measured);
            self.widths[column] = width;
            self.known_widths.insert(candidate.column.clone(), width);
            cx.emit(GridEvent::WidthChanged {
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
                let measured = window
                    .text_system()
                    .shape_line(
                        text.clone().into(),
                        font_size,
                        &[text_style.to_run(text.len())],
                        None,
                    )
                    .width()
                    .as_f32();
                let width = inferred_width(measured);
                if width > self.widths[column] {
                    self.widths[column] = width;
                    let name = self.result.columns[column].0.clone();
                    self.known_widths.insert(name.clone(), width);
                    cx.emit(GridEvent::WidthChanged {
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
            self.keep_active_column_visible(viewport_width);
        }
        if processed_candidates {
            self.width_batch_blocked = true;
            let grid = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                let _ = grid.update(cx, |grid, cx| {
                    grid.width_batch_blocked = false;
                    if !grid.width_candidates.is_empty() {
                        cx.notify();
                    }
                });
            });
        }
        self.typography = Some((font.clone(), font_size));
        self.row_height = row_height;
        let geometry = GridGeometry::new(
            self.widths.clone(),
            self.frozen_count,
            viewport_width,
            self.scroll_x,
        );
        let (frozen, scrollable) = geometry.visible_columns(180.0);
        let visible = scrollable.chain(frozen).collect::<Vec<_>>();
        let frozen_count = self.frozen_count;
        let result = self.result.clone();
        let columns = self.result.columns.clone();
        let dtypes = self.result.dtypes.clone();
        let row_count = self.result.row_count();
        let scroll_y = -self
            .vertical_scroll
            .0
            .borrow()
            .base_handle
            .offset()
            .y
            .as_f32();
        let first_visible_row = (scroll_y / row_height) as usize;
        let first_page = first_visible_row / crate::domain::frame::PAGE_SIZE;
        let page_count = row_count.div_ceil(crate::domain::frame::PAGE_SIZE);
        for page in first_page..(first_page + 2).min(page_count) {
            self.prefetch_page(page, cx);
        }
        if self.header_cache.as_ref().is_none_or(|cache| {
            cache.font != font
                || cache.font_size != font_size
                || cache.line_height != line_height
                || cache.rem_size != rem_size
        }) {
            let mut dtype_style = text_style.clone();
            dtype_style.font_size = rems(0.75).into();
            let dtype_line_height = dtype_style.line_height_in_pixels(rem_size).as_f32();
            let labels = columns
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
            let height = columns
                .iter()
                .enumerate()
                .map(|(column, _)| {
                    let width = (self.widths[column] - 17.0).max(1.0);
                    let dtype = type_label(&dtypes[column]);
                    9.0 + wrapped_line_count(window, &labels[column], width, &text_style) as f32
                        * line_height.as_f32()
                        + wrapped_line_count(window, &dtype, width, &dtype_style) as f32
                            * dtype_line_height
                })
                .fold(48.0_f32, f32::max)
                .ceil();
            self.header_cache = Some(HeaderCache {
                font,
                font_size,
                line_height,
                rem_size,
                labels,
                height,
            });
        }
        let header_cache = self.header_cache.as_ref().unwrap();
        let header_height = header_cache.height;
        let labels = &header_cache.labels;
        let background = rgba(self.theme.background.rgba());
        let foreground = rgba(self.theme.foreground.rgba());
        let head = rgba(self.theme.header_background.rgba());
        let frozen_head = rgba(self.theme.frozen_header_background.rgba());
        let even = rgba(self.theme.row_stripe_background.rgba());
        let active = rgba(self.theme.selection_background.rgba());
        let active_border = rgba(self.theme.active_cell_border.rgba());
        let grid_border = rgba(self.theme.grid_border.rgba());
        let frozen_background = rgba(self.theme.frozen_background.rgba());
        let frozen_stripe = rgba(self.theme.frozen_stripe_background.rgba());
        let h_viewport = geometry.scrollable_width();
        let h_content = geometry.content_width();
        let h_max = geometry.max_scroll_x();
        let h_thumb = if h_content > h_viewport && h_viewport > 0.0 {
            (h_viewport * h_viewport / h_content).clamp(MIN_THUMB_LEN.min(h_viewport), h_viewport)
        } else {
            0.0
        };
        let h_stack = (h_viewport - h_thumb).max(0.0);
        let h_pos = if h_max > 0.0 {
            self.scroll_x / h_max * h_stack
        } else {
            0.0
        };
        let table_bounds = self.vertical_scroll.0.borrow().base_handle.bounds();
        let table_left = if table_bounds.size.width > px(0.) {
            table_bounds.left()
        } else {
            window.bounds().left()
        };
        let track_left = f32::from(table_left) + geometry.frozen_width();
        let horizontal_bar = if h_thumb > 0.0 {
            let move_entity = cx.entity();
            let up_entity = cx.entity();
            let drag_canvas = canvas(
                move |_bounds, _window, _cx| (),
                move |bounds, _state, window, _cx| {
                    let canvas_left = f32::from(bounds.left());
                    let move_entity = move_entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, _phase, _window, cx| {
                        if event.pressed_button != Some(MouseButton::Left) {
                            return;
                        }
                        let local = f32::from(event.position.x) - canvas_left;
                        move_entity.update(cx, |this, cx| {
                            let Some(grab) = this.h_scroll_drag else {
                                return;
                            };
                            if h_stack <= 0.0 {
                                return;
                            }
                            this.scroll_x = ((local - grab).clamp(0.0, h_stack) / h_stack * h_max)
                                .clamp(0.0, h_max);
                            cx.notify();
                        });
                    });
                    window.on_mouse_event(move |_event: &MouseUpEvent, _phase, _window, cx| {
                        up_entity.update(cx, |this, cx| {
                            if this.h_scroll_drag.take().is_some() {
                                cx.notify();
                            }
                        });
                    });
                },
            )
            .size_full();
            div()
                .id("grid-h-scrollbar")
                .relative()
                .h(px(SCROLLBAR_WIDTH))
                .w(px(h_viewport))
                .ml(px(geometry.frozen_width()))
                .flex_shrink_0()
                .cursor_pointer()
                .hover(|style| style.bg(gpui_kit::rgba(0x0000000D)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        if h_stack <= 0.0 {
                            return;
                        }
                        let local = f32::from(event.position.x) - track_left;
                        let (grab, jump) = if local >= h_pos && local <= h_pos + h_thumb {
                            (local - h_pos, None)
                        } else {
                            let pos = (local - h_thumb / 2.0).clamp(0.0, h_stack);
                            (h_thumb / 2.0, Some(pos / h_stack * h_max))
                        };
                        this.h_scroll_drag = Some(grab);
                        if let Some(offset) = jump {
                            this.scroll_x = offset;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(h_pos))
                        .top(px((SCROLLBAR_WIDTH - SCROLLBAR_THUMB) / 2.0))
                        .w(px(h_thumb))
                        .h(px(SCROLLBAR_THUMB))
                        .bg(gpui_kit::rgba(0x00000033))
                        .hover(|style| style.bg(gpui_kit::rgba(0x00000066)))
                        .rounded_sm(),
                )
                .child(drag_canvas)
                .into_any_element()
        } else {
            div().h(px(0.0)).into_any_element()
        };
        let vertical_bar = Scrollbar::vertical(&self.vertical_scroll)
            .id("grid-v-scrollbar")
            .mode(ScrollbarMode::Always)
            .styles(|styles| {
                styles
                    .track(|style| style.width(px(SCROLLBAR_WIDTH)))
                    .thumb(|style| {
                        style
                            .width(px(SCROLLBAR_THUMB))
                            .min_length(px(MIN_THUMB_LEN))
                    })
            });
        let move_entity = cx.entity();
        let up_entity = move_entity.clone();
        let selection_drag_canvas = canvas(
            move |_bounds, _window, _cx| (),
            move |_bounds, _state, window, _cx| {
                let move_entity = move_entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _phase, _window, cx| {
                    if event.pressed_button != Some(MouseButton::Left) {
                        return;
                    }
                    let position = event.position;
                    move_entity.update(cx, |grid, cx| grid.extend_selection_drag(position, cx));
                });
                window.on_mouse_event(move |_event: &MouseUpEvent, _phase, _window, cx| {
                    up_entity.update(cx, |grid, cx| {
                        if grid.selection_drag.take().is_some() {
                            cx.notify();
                        }
                    });
                });
            },
        )
        .absolute()
        .left_0()
        .right_0()
        .top_0()
        .bottom_0()
        .size_full();
        let header = visible
            .iter()
            .filter_map(|&column| {
                geometry.column_x(column).map(|x| {
                    let name = columns[column].0.clone();
                    let right_click_name = name.clone();
                    let dtype = type_label(&dtypes[column]).into_owned();
                    let is_frozen = column < frozen_count;
                    let cell = div()
                        .id(format!("grid-header:{column}"))
                        .absolute()
                        .left(px(if is_frozen {
                            x
                        } else {
                            x - geometry.frozen_width()
                        }))
                        .top_0()
                        .h(px(header_height))
                        .w(px(self.widths[column]))
                        .px_2()
                        .py_1()
                        .border_r_1()
                        .border_color(grid_border)
                        .overflow_hidden()
                        .when(is_frozen, |cell| cell.bg(frozen_head))
                        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            cx.emit(GridEvent::SortRequested(name.clone()))
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |_, _, _, cx| {
                                cx.emit(GridEvent::ClearSortRequested(right_click_name.clone()))
                            }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(labels[column].clone())
                                .child(div().text_xs().child(dtype)),
                        );
                    #[cfg(feature = "snapshots")]
                    let cell = cell.test_support();
                    (is_frozen, cell)
                })
            })
            .collect::<Vec<_>>();
        let mut frozen_header = Vec::new();
        let mut scrollable_header = Vec::new();
        for (is_frozen, cell) in header {
            if is_frozen {
                frozen_header.push(cell);
            } else {
                scrollable_header.push(cell);
            }
        }
        let scrollable_header_pane = div()
            .id("grid-scrollable-header-pane")
            .absolute()
            .left(px(geometry.frozen_width()))
            .right_0()
            .top_0()
            .h(px(header_height))
            .overflow_hidden()
            .children(scrollable_header);
        #[cfg(feature = "snapshots")]
        let scrollable_header_pane = scrollable_header_pane.test_support();
        let header_row = div()
            .id("grid-header-row")
            .relative()
            .h(px(header_height))
            .flex_shrink_0()
            .w_full()
            .border_b_1()
            .border_color(grid_border)
            .bg(head)
            .overflow_hidden()
            .child(scrollable_header_pane)
            .children(frozen_header);
        #[cfg(feature = "snapshots")]
        let header_row = header_row.test_support();
        let footer_row = self.footer.as_ref().map(|footer| {
            let mut frozen_cells = Vec::new();
            let mut scrollable_cells = Vec::new();
            for &column in &visible {
                let Some(x) = geometry.column_x(column) else {
                    continue;
                };
                let is_frozen = column < frozen_count;
                let value = footer.value(&columns[column].0);
                let text = match value {
                    Some(value) => {
                        cell_preview(value.as_deref(), &dtypes[column], self.formats[column]).0
                    }
                    None if column == 0 => footer.label.clone(),
                    None => String::new(),
                };
                let cell = div()
                    .id(format!("grid-footer:{column}"))
                    .absolute()
                    .left(px(if is_frozen {
                        x
                    } else {
                        x - geometry.frozen_width()
                    }))
                    .top_0()
                    .h(px(row_height))
                    .w(px(self.widths[column]))
                    .px_2()
                    .border_r_1()
                    .border_color(grid_border)
                    .overflow_hidden()
                    .truncate()
                    .when(value.is_some() && self.formats[column].is_some(), |cell| {
                        cell.text_right()
                    })
                    .when(is_frozen, |cell| cell.bg(frozen_head))
                    .aria_label(text.clone())
                    .child(text);
                #[cfg(feature = "snapshots")]
                let cell = cell.test_support();
                if is_frozen {
                    frozen_cells.push(cell);
                } else {
                    scrollable_cells.push(cell);
                }
            }
            div()
                .id("grid-footer-row")
                .relative()
                .h(px(row_height))
                .flex_shrink_0()
                .w_full()
                .border_t_1()
                .border_color(grid_border)
                .bg(head)
                .font_weight(FontWeight::SEMIBOLD)
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .left(px(geometry.frozen_width()))
                        .right_0()
                        .top_0()
                        .h(px(row_height))
                        .overflow_hidden()
                        .children(scrollable_cells),
                )
                .children(frozen_cells)
        });
        div()
            .id("grid-root")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(background)
            .text_color(foreground)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|grid, event: &KeyDownEvent, window, cx| {
                let mut parts = Vec::new();
                if event.keystroke.modifiers.control {
                    parts.push("ctrl".to_owned());
                }
                if event.keystroke.modifiers.alt {
                    parts.push("alt".to_owned());
                }
                if event.keystroke.modifiers.shift {
                    parts.push("shift".to_owned());
                }
                if event.keystroke.modifiers.platform {
                    parts.push("cmd".to_owned());
                }
                parts.push(event.keystroke.key.to_ascii_lowercase());
                let Ok(stroke) = canonical_keystroke(&parts.join("-")) else {
                    return;
                };
                // Windows reports shifted punctuation with Shift still set. Match the
                // produced character as well as explicitly rebound physical keystrokes.
                let modifiers = event.keystroke.modifiers;
                let produced = if !modifiers.control && !modifiers.alt && !modifiers.platform {
                    event
                        .keystroke
                        .key_char
                        .as_deref()
                        .filter(|key| matches!(*key, "(" | ")" | "<" | ">"))
                        .or(match (modifiers.shift, event.keystroke.key.as_str()) {
                            (true, "0" | ")") => Some(")"),
                            (true, "9" | "(") => Some("("),
                            (true, "," | "<") => Some("<"),
                            (true, "." | ">") => Some(">"),
                            _ => None,
                        })
                } else {
                    None
                };
                let action = grid
                    .keymap
                    .binding_for(&stroke, grid.key_context)
                    .or_else(|| {
                        produced.and_then(|key| grid.keymap.binding_for(key, grid.key_context))
                    });
                let Some(action) = action else {
                    return;
                };
                if action == ActionId::IncreaseColumnWidth
                    || action == ActionId::DecreaseColumnWidth
                {
                    grid.change_width(action == ActionId::IncreaseColumnWidth, window, cx);
                } else if matches!(
                    action,
                    ActionId::ToggleThousandsGrouping
                        | ActionId::IncreaseDecimalPrecision
                        | ActionId::DecreaseDecimalPrecision
                ) {
                    grid.change_numeric_format(action, cx);
                } else if action == ActionId::MoveColumnLeft {
                    cx.emit(GridEvent::MoveColumn(-1));
                } else if action == ActionId::MoveColumnRight {
                    cx.emit(GridEvent::MoveColumn(1));
                } else if action == ActionId::SortAscending {
                    cx.emit(GridEvent::SortDirectionRequested(false));
                } else if action == ActionId::SortDescending {
                    cx.emit(GridEvent::SortDirectionRequested(true));
                } else if is_grid_navigation_action(action) {
                    grid.navigate_action(action, window, cx);
                } else {
                    return;
                }
                cx.stop_propagation();
            }))
            .on_scroll_wheel(cx.listener(|grid, event: &ScrollWheelEvent, window, cx| {
                let delta = event.delta.pixel_delta(px(grid.row_height));
                let horizontal = if event.modifiers.shift {
                    delta.y.as_f32()
                } else {
                    delta.x.as_f32()
                };
                if horizontal != 0.0 {
                    grid.scroll_horizontal(-horizontal, grid.viewport_width(window), cx);
                    cx.stop_propagation();
                }
            }))
            .child(selection_drag_canvas)
            .child(header_row)
            .child(if row_count == 0 {
                div()
                    .flex_1()
                    .px_3()
                    .py_3()
                    .child("No rows to display")
                    .into_any_element()
            } else {
                let widths = self.widths.clone();
                let formats = self.formats.clone();
                let heat: Rc<[Option<(f64, f64)>]> = columns
                    .iter()
                    .map(|column| self.heat_ranges.get(&column.0).copied())
                    .collect();
                let selection = self.selection;
                let highlight_terms: Rc<[Vec<String>]> = if self.highlights.is_empty() {
                    Rc::from([])
                } else {
                    let names = columns
                        .iter()
                        .map(|column| column.0.as_str())
                        .collect::<Vec<_>>();
                    self.highlights.for_columns(&names).into()
                };
                uniform_list(
                    "dataframe-rows",
                    row_count,
                    cx.processor(move |grid, rows: std::ops::Range<usize>, window, cx| {
                        let _cells = crate::infrastructure::performance::Span::new("grid_cells");
                        let page_rows = Rc::new(
                            result
                                .cached_rows(rows.start, rows.len())
                                .unwrap_or_default(),
                        );
                        if crate::infrastructure::performance::enabled()
                            && page_rows.len() == rows.len()
                        {
                            grid.benchmark_rendered_revision = Some(result.result_revision);
                        }
                        let page_start = rows.start;
                        let column_indices = result.column_indices.clone();
                        rows.map(|row| {
                            let cells = visible
                                .iter()
                                .filter_map(|&column| {
                                    geometry.column_x(column).map(|x| {
                                        let is_frozen = column < frozen_count;
                                        let value = page_rows
                                            .get(row.saturating_sub(page_start))
                                            .and_then(|row| row.values.get(column_indices[column]))
                                            .and_then(Option::as_deref);
                                        let (text, hidden) =
                                            cell_preview(value, &dtypes[column], formats[column]);
                                        let selected = selection.is_some_and(|selection| {
                                            let (rows, columns) = selection.rectangle();
                                            rows.contains(&row) && columns.contains(&column)
                                        });
                                        let active_cell = selection.is_some_and(|selection| {
                                            selection.active == Cell { row, column }
                                        });
                                        let frozen_row_background = if row % 2 == 1 {
                                            frozen_stripe
                                        } else {
                                            frozen_background
                                        };
                                        let is_numeric = formats[column].is_some();
                                        let shade = heat[column].and_then(|range| {
                                            let value = value?.parse::<f64>().ok()?;
                                            Some(heat_color(crate::domain::heatmap::fraction(
                                                value, range,
                                            )))
                                        });
                                        let is_temporal = temporal_type_name(&dtypes[column]);
                                        let available_width = (widths[column]
                                            - rem_size.as_f32()
                                            - if active_cell { 4.0 } else { 1.0 })
                                        .max(0.0);
                                        let overflow = hidden
                                            || window
                                                .text_system()
                                                .shape_line(
                                                    text.clone().into(),
                                                    font_size,
                                                    &[text_style.to_run(text.len())],
                                                    None,
                                                )
                                                .width()
                                                .as_f32()
                                                > available_width;
                                        // A new publication or format must retire an already-open tooltip.
                                        let content = div()
                                            .id(format!(
                                                "grid-cell-text:{row}:{column}:{}:{}:{:?}",
                                                result.result_revision,
                                                result.generation,
                                                formats[column]
                                            ))
                                            .size_full()
                                            .truncate()
                                            .aria_label(text.clone())
                                            .when(overflow, |content| {
                                                let page_rows = page_rows.clone();
                                                let source_column = column_indices[column];
                                                let dtype = dtypes[column].clone();
                                                let format = formats[column];
                                                content.hoverable_tooltip(move |window, cx| {
                                                    let value = page_rows
                                                        .get(row - page_start)
                                                        .and_then(|row| {
                                                            row.values.get(source_column)
                                                        })
                                                        .and_then(Option::as_deref);
                                                    cell_tooltip(
                                                        display_cell(
                                                            value,
                                                            &dtype,
                                                            format,
                                                            usize::MAX,
                                                        ),
                                                        window,
                                                        cx,
                                                    )
                                                })
                                            })
                                            .child({
                                                let ranges = crate::domain::query::highlight_ranges(
                                                    &text,
                                                    highlight_terms
                                                        .get(column)
                                                        .map_or(&[], Vec::as_slice),
                                                );
                                                let highlight = HighlightStyle {
                                                    background_color: Some(
                                                        rgba(SEARCH_HIGHLIGHT).into(),
                                                    ),
                                                    ..Default::default()
                                                };
                                                StyledText::new(text).with_highlights(
                                                    ranges
                                                        .into_iter()
                                                        .map(|range| (range, highlight)),
                                                )
                                            });
                                        #[cfg(feature = "snapshots")]
                                        let content = content.test_support();
                                        let cell = div()
                                            .id(format!("grid-cell:{row}:{column}"))
                                            .absolute()
                                            .left(px(if is_frozen {
                                                x
                                            } else {
                                                x - geometry.frozen_width()
                                            }))
                                            .top_0()
                                            .h(px(row_height))
                                            .w(px(widths[column]))
                                            .px_2()
                                            .border_r_1()
                                            .border_color(grid_border)
                                            .overflow_hidden()
                                            .when(is_numeric, |cell| cell.text_right())
                                            .when(is_temporal, |cell| cell.text_center())
                                            .when(is_frozen, |cell| cell.bg(frozen_row_background))
                                            .when_some(shade, |cell, shade| cell.bg(shade))
                                            .when(selected, |cell| cell.bg(active))
                                            .when(active_cell, |cell| {
                                                cell.border_2().border_color(active_border)
                                            })
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(
                                                    move |grid,
                                                          event: &MouseDownEvent,
                                                          window,
                                                          cx| {
                                                        grid.begin_selection_drag(
                                                            Cell { row, column },
                                                            event.modifiers.shift,
                                                            cx,
                                                        );
                                                        window.focus(&grid.focus, cx);
                                                    },
                                                ),
                                            )
                                            .child(content);
                                        #[cfg(feature = "snapshots")]
                                        let cell = cell.test_support();
                                        (is_frozen, cell)
                                    })
                                })
                                .collect::<Vec<_>>();
                            let mut frozen_cells = Vec::new();
                            let mut scrollable_cells = Vec::new();
                            for (is_frozen, cell) in cells {
                                if is_frozen {
                                    frozen_cells.push(cell);
                                } else {
                                    scrollable_cells.push(cell);
                                }
                            }
                            let scrollable_row_pane = div()
                                .id(format!("grid-scrollable-row:{row}"))
                                .absolute()
                                .left(px(geometry.frozen_width()))
                                .right_0()
                                .top_0()
                                .h(px(row_height))
                                .overflow_hidden()
                                .children(scrollable_cells);
                            #[cfg(feature = "snapshots")]
                            let scrollable_row_pane = scrollable_row_pane.test_support();
                            div()
                                .relative()
                                .h(px(row_height))
                                .w_full()
                                .overflow_hidden()
                                .when(row % 2 == 1, |row| row.bg(even))
                                .child(scrollable_row_pane)
                                .children(frozen_cells)
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .track_scroll(&self.vertical_scroll)
                .into_any_element()
            })
            .children(footer_row)
            .child(horizontal_bar)
            .when(row_count > 0, |grid| grid.child(vertical_bar))
    }
}

#[cfg(all(test, feature = "snapshots"))]
mod tests {
    use super::GridView;
    use crate::domain::frame::{FrameSnapshot, ResultSnapshot};
    use crate::infrastructure::duckdb_store::DuckDbStore;
    use duckdb::types::Value;
    use gpui_kit::component::Root;
    use gpui_kit::test::{TestAppContextExt, TestWindowExt};
    use gpui_kit::{
        AppContext, Context, Entity, IntoElement, ParentElement, Render, ScrollDelta, Styled,
        TestAppContext, Window, div, point, px, size,
    };
    use std::time::Duration;

    fn text_grid(
        values: Vec<duckdb::types::Value>,
        cx: &mut TestAppContext,
    ) -> (gpui_kit::AnyWindowHandle, Entity<GridView>) {
        cx.update(gpui_kit::init);
        let mut grid = None;
        let handle = cx.open_window(size(px(500.), px(350.)), |window, cx| {
            let source = FrameSnapshot::new(
                DuckDbStore::from_rows(
                    &[("Text".into(), "VARCHAR".into())],
                    values.into_iter().map(|value| vec![value]),
                )
                .unwrap(),
                1,
            );
            let result = ResultSnapshot::from_source(&source, 1, 1);
            result.prefetch_page(0).unwrap();
            let view = cx.new(|cx| GridView::new(result, cx));
            view.update(cx, |grid, cx| {
                grid.widths[0] = 100.;
                grid.set_frozen_count(1, cx);
            });
            grid = Some(view.clone());
            Root::new(view, window, cx)
        });
        (handle.into(), grid.unwrap())
    }

    #[gpui_kit::test]
    async fn long_cells_stay_on_one_line_and_hover_reveals_scrollable_full_text(
        cx: &mut TestAppContext,
    ) {
        let long = format!(
            "{}THE END",
            "Long Unicode text 界😀\twith details\n".repeat(60)
        );
        let (handle, grid) = text_grid(
            vec![
                Value::Text("short".into()),
                Value::Text(long.clone()),
                Value::Null,
            ],
            cx,
        );
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let first = window.find("grid-cell:0:0").bounds();
            let second = window.find("grid-cell:1:0").bounds();
            assert_eq!(first.size.height, second.size.height);
            assert_eq!(second.top(), first.bottom());
            let preview = window.find("grid-cell-text:1:0:1:1:None");
            let label = preview.label().unwrap();
            assert!(!label.contains(['\n', '\t']));
            assert!(label.ends_with('…'));
            window.hover("grid-cell:1:0", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(2), |window, _| {
            window.try_find("grid-cell-tooltip-value").is_some()
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            assert_eq!(
                window.find("grid-cell-tooltip-value").label(),
                Some(long.as_str())
            );
            let scroll = window.find("grid-cell-tooltip-scroll").bounds();
            assert!(scroll.size.width <= px(500.));
            assert!(scroll.size.height <= px(350.));
            let top = window.find("grid-cell-tooltip-value").bounds().top();
            let focus = grid.read(cx).focus.clone();
            window.focus(&focus, cx);
            window.hover("grid-cell-tooltip-scroll", cx);
            window.scroll(
                "grid-cell-tooltip-scroll",
                ScrollDelta::Pixels(point(px(0.), -px(150.))),
                cx,
            );
            assert!(window.find("grid-cell-tooltip-value").bounds().top() < top);
            assert!(focus.is_focused(window));
            window.simulate_mouse_move(point(px(495.), px(345.)), cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(2), |window, _| {
            window.try_find("grid-cell-tooltip-value").is_none()
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            window.hover("grid-cell:0:0", cx);
        })
        .unwrap();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("grid-cell-tooltip-value").is_none());
            window.hover("grid-cell:2:0", cx);
        })
        .unwrap();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("grid-cell-tooltip-value").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn hover_tracks_replaced_results_and_current_width(cx: &mut TestAppContext) {
        let (handle, grid) = text_grid(
            vec![
                Value::Text("WWWWWWWWWWWWWWWWWWWW".into()),
                Value::Text("another value".into()),
            ],
            cx,
        );
        cx.update_window(handle, |_, window, cx| {
            grid.update(cx, |grid, cx| {
                grid.widths[0] = 40.;
                cx.notify();
            });
            window.render_frame(cx);
            window.hover("grid-cell:0:0", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(2), |window, _| {
            window.try_find("grid-cell-tooltip-value").is_some()
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            assert_eq!(
                window.find("grid-cell-tooltip-value").label(),
                Some("WWWWWWWWWWWWWWWWWWWW")
            );
            grid.update(cx, |grid, cx| {
                let source = FrameSnapshot::new(
                    DuckDbStore::from_rows(
                        &[("Text".into(), "VARCHAR".into())],
                        [vec![Value::Text("Replacement value".into())]],
                    )
                    .unwrap(),
                    2,
                );
                let result = ResultSnapshot::from_source(&source, 2, 2);
                result.prefetch_page(0).unwrap();
                grid.replace_result(result, cx);
            });
            window.render_frame(cx);
            window.hover("grid-cell:0:0", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(2), |window, _| {
            window
                .try_find("grid-cell-tooltip-value")
                .is_some_and(|value| value.label() == Some("Replacement value"))
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            grid.update(cx, |grid, cx| {
                grid.widths[0] = 400.;
                cx.notify();
            });
            window.render_frame(cx);
            window.hover("grid-cell:0:0", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(2), |window, _| {
            window.try_find("grid-cell-tooltip-value").is_none()
        })
        .await;
    }

    #[gpui_kit::test]
    async fn hover_resolves_sorted_rows_and_reordered_columns(cx: &mut TestAppContext) {
        use crate::domain::documents::SortKey;
        use crate::infrastructure::duckdb_store::ViewQuery;

        cx.update(gpui_kit::init);
        let original = "original row ".repeat(30);
        let sorted = "sorted row ".repeat(30);
        let source = FrameSnapshot::new(
            DuckDbStore::from_rows(
                &[
                    ("Key".into(), "VARCHAR".into()),
                    ("Text".into(), "VARCHAR".into()),
                    ("Amount".into(), "DOUBLE".into()),
                ],
                [
                    vec![
                        Value::Text("z".into()),
                        Value::Text(original.clone()),
                        Value::Double(1.),
                    ],
                    vec![
                        Value::Text("a".into()),
                        Value::Text(sorted.clone()),
                        Value::Double(987654321.123),
                    ],
                ],
            )
            .unwrap(),
            1,
        );
        let result = ResultSnapshot::from_source(&source, 1, 1);
        result.prefetch_page(0).unwrap();
        let mut grid = None;
        let handle = cx.open_window(size(px(500.), px(350.)), |window, cx| {
            let view = cx.new(|cx| GridView::new(result, cx));
            grid = Some(view.clone());
            Root::new(view, window, cx)
        });
        let handle = handle.into();
        let grid = grid.unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.hover("grid-cell:0:1", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(2), |window, _| {
            window
                .try_find("grid-cell-tooltip-value")
                .is_some_and(|value| value.label() == Some(original.as_str()))
        })
        .await;
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
        cx.update_window(handle, |_, window, cx| {
            grid.update(cx, |grid, cx| {
                grid.replace_result(result, cx);
                grid.widths[0] = 40.;
                grid.set_frozen_count(1, cx);
            });
            window.render_frame(cx);
            window.hover("grid-cell:0:1", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(2), |window, _| {
            window
                .try_find("grid-cell-tooltip-value")
                .is_some_and(|value| value.label() == Some(sorted.as_str()))
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            window.simulate_mouse_move(point(px(495.), px(345.)), cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(2), |window, _| {
            window.try_find("grid-cell-tooltip-value").is_none()
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            window.hover("grid-cell:0:0", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(2), |window, _| {
            window
                .try_find("grid-cell-tooltip-value")
                .is_some_and(|value| value.label() == Some("987,654,321.12"))
        })
        .await;
    }

    #[gpui_kit::test]
    fn inference_measures_four_columns_per_frame_and_reset_rejects_old_candidates(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let mut grid = None;
        let handle = cx.open_window(size(px(800.), px(400.)), |window, cx| {
            let columns = (0..6)
                .map(|index| (format!("c{index}"), "VARCHAR".into()))
                .collect::<Vec<_>>();
            let source = FrameSnapshot::new(
                DuckDbStore::from_rows(&columns, [vec![Value::Text("界 WWW".into()); 6]]).unwrap(),
                1,
            );
            let view = cx.new(|cx| GridView::new(ResultSnapshot::from_source(&source, 1, 1), cx));
            view.update(cx, |grid, cx| {
                grid.configure_widths("source".into(), Some(source), Default::default(), cx)
            });
            grid = Some(view.clone());
            Root::new(view, window, cx)
        });
        let grid = grid.unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            grid.update(cx, |grid, _| {
                grid.reset_widths();
                for index in 0..6 {
                    let column = format!("c{index}");
                    grid.pending_widths.insert(column.clone());
                    grid.width_candidates.push_back(super::WidthCandidates {
                        column,
                        samples: vec!["WWW".into()],
                        epoch: grid.width_epoch,
                        revision: 0,
                    });
                }
            });
            window.render_frame(cx);
            assert_eq!(grid.read(cx).known_widths.len(), 4);
            assert_eq!(grid.read(cx).pending_widths.len(), 2);
            window.render_frame(cx);
            assert_eq!(grid.read(cx).known_widths.len(), 4);
            window.simulate_next_frame(cx);
            window.render_frame(cx);
            assert_eq!(grid.read(cx).known_widths.len(), 6);
            assert!(
                grid.read(cx)
                    .widths
                    .iter()
                    .all(|width| (80.0..=600.0).contains(width))
            );
            grid.update(cx, |grid, _| {
                let old_epoch = grid.width_epoch;
                grid.reset_widths();
                grid.width_candidates.push_back(super::WidthCandidates {
                    column: "c0".into(),
                    samples: vec!["W".repeat(200)],
                    epoch: old_epoch,
                    revision: 0,
                });
            });
            window.render_frame(cx);
            assert_eq!(grid.read(cx).column_width("c0"), 200.);
            let focus = grid.read(cx).focus.clone();
            window.focus(&focus, cx);
            grid.update(cx, |grid, _| grid.widths[0] = 1195.);
            window.press(")", cx);
            assert_eq!(grid.read(cx).column_width("c0"), 1200.);
            grid.update(cx, |grid, _| grid.widths[0] = 45.);
            window.press("(", cx);
            assert_eq!(grid.read(cx).column_width("c0"), 40.);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn vertical_thumb_tracks_drag_wheel_and_keyboard_in_an_inset_grid(cx: &mut TestAppContext) {
        struct InsetGrid(Entity<GridView>);
        impl Render for InsetGrid {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(div().h(px(70.)).flex_shrink_0())
                    .child(div().flex_1().min_h_0().child(self.0.clone()))
            }
        }

        cx.update(gpui_kit::init);
        let mut grid = None;
        let handle = cx.open_window(size(px(500.), px(350.)), |window, cx| {
            let source = FrameSnapshot::new(
                DuckDbStore::from_rows(
                    &[("ID".into(), "BIGINT".into())],
                    (0..1000).map(|row| vec![Value::BigInt(row)]),
                )
                .unwrap(),
                1,
            );
            let view = cx.new(|cx| GridView::new(ResultSnapshot::from_source(&source, 1, 1), cx));
            grid = Some(view.clone());
            let inset = cx.new(|_| InsetGrid(view));
            Root::new(inset, window, cx)
        });
        let grid = grid.unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let scroll = grid.read(cx).vertical_scroll.0.borrow().base_handle.clone();
            let viewport = scroll.bounds();
            assert!(viewport.top() >= px(118.));
            assert!(viewport.size.height < px(250.));
            let top = point(viewport.right() - px(6.), viewport.top() + px(16.));
            let middle = point(top.x, viewport.center().y);
            window.drag(top, middle, cx);
            let offset = scroll.offset().y;
            assert!(offset < -px(1000.), "drag must scroll the rows");

            // Grabbing the moved thumb again must not jump back to another position.
            window.drag(middle, middle, cx);
            assert!((scroll.offset().y - offset).abs() < px(1.));
            window.drag(middle, top, cx);
            assert!(scroll.offset().y.abs() < px(1.));

            window.scroll(
                "grid-cell:0:0",
                ScrollDelta::Pixels(point(px(0.), -px(10000.))),
                cx,
            );
            assert!(scroll.offset().y < px(0.));
            let wheel_offset = scroll.offset().y;
            let wheel_thumb = point(
                top.x,
                viewport.top()
                    + px(12.)
                    + (viewport.size.height - px(24.)) * (-wheel_offset / scroll.max_offset().y),
            );
            window.drag(wheel_thumb, wheel_thumb, cx);
            assert!((scroll.offset().y - wheel_offset).abs() < px(1.));

            let focus = grid.read(cx).focus.clone();
            window.focus(&focus, cx);
            window.press("ctrl-end", cx);
            assert_eq!(grid.read(cx).selection().unwrap().active.row, 999);
            assert!((-scroll.offset().y - scroll.max_offset().y).abs() < px(1.));
            let bottom = point(top.x, viewport.bottom() - px(16.));
            let end_offset = scroll.offset().y;
            window.drag(bottom, bottom, cx);
            assert!((scroll.offset().y - end_offset).abs() < px(1.));
            window.drag(bottom, point(top.x, viewport.top()), cx);
            assert!(scroll.offset().y.abs() < px(1.));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn dragging_across_cells_selects_a_rectangle_without_scrolling_the_view(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let mut grid = None;
        let handle = cx.open_window(size(px(500.), px(350.)), |window, cx| {
            let source = FrameSnapshot::new(
                DuckDbStore::from_rows(
                    &[
                        ("A".into(), "BIGINT".into()),
                        ("B".into(), "BIGINT".into()),
                        ("C".into(), "BIGINT".into()),
                    ],
                    (0..100).map(|row| {
                        vec![Value::BigInt(row), Value::BigInt(row), Value::BigInt(row)]
                    }),
                )
                .unwrap(),
                1,
            );
            let view = cx.new(|cx| GridView::new(ResultSnapshot::from_source(&source, 1, 1), cx));
            grid = Some(view.clone());
            Root::new(view, window, cx)
        });
        let grid = grid.unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let scroll = grid.read(cx).vertical_scroll.0.borrow().base_handle.clone();
            let start = window.find("grid-cell:0:0").bounds().center();
            let end = window.find("grid-cell:4:2").bounds().center();
            window.drag(start, end, cx);

            let selection = grid.read(cx).selection().unwrap();
            assert_eq!(
                selection.anchor,
                crate::domain::selection::Cell { row: 0, column: 0 }
            );
            assert_eq!(
                selection.active,
                crate::domain::selection::Cell { row: 4, column: 2 }
            );
            assert_eq!(scroll.offset().y, px(0.));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn frozen_pane_stays_unchanged_when_scrolling_and_header_wraps(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let mut grid = None;
        let handle = cx.open_window(size(px(500.), px(350.)), |window, cx| {
            let source = FrameSnapshot::new(
                DuckDbStore::from_rows(
                    &[
                        ("ID".into(), "VARCHAR".into()),
                        (
                            "a_long_column_name_that_needs_several_lines_in_the_header".into(),
                            "VARCHAR".into(),
                        ),
                        ("third".into(), "VARCHAR".into()),
                        ("fourth".into(), "VARCHAR".into()),
                    ],
                    [
                        vec![
                            Value::Text("one".into()),
                            Value::Text("scrolling content one".into()),
                            Value::Text("three".into()),
                            Value::Text("five".into()),
                        ],
                        vec![
                            Value::Text("two".into()),
                            Value::Text("scrolling content two".into()),
                            Value::Text("four".into()),
                            Value::Text("six".into()),
                        ],
                    ],
                )
                .unwrap(),
                1,
            );
            let view = cx.new(|cx| GridView::new(ResultSnapshot::from_source(&source, 1, 1), cx));
            grid = Some(view.clone());
            Root::new(view, window, cx)
        });
        let grid = grid.unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            grid.update(cx, |grid, cx| grid.set_frozen_count(1, cx));
            window.render_frame(cx);
            let header = window.find("grid-header-row").bounds();
            let frozen_header = window.find("grid-header:0").bounds();
            let scrolling_header = window.find("grid-header:1").bounds();
            let frozen_cell = window.find("grid-cell:0:0").bounds();
            let header_pane = window.find("grid-scrollable-header-pane").bounds();
            let row_pane = window.find("grid-scrollable-row:0").bounds();
            assert!(header.size.height.as_f32() > 48.0);
            assert_eq!(
                header_pane.origin.x,
                frozen_header.origin.x + frozen_header.size.width
            );
            assert_eq!(
                row_pane.origin.x,
                frozen_cell.origin.x + frozen_cell.size.width
            );
            assert!(frozen_cell.origin.y >= header.origin.y + header.size.height);

            grid.update(cx, |grid, cx| grid.scroll_horizontal(120.0, 500.0, cx));
            window.render_frame(cx);
            assert_eq!(grid.read(cx).scroll_x, 120.0);
            assert_eq!(
                window.find("grid-header:1").bounds().origin.x,
                scrolling_header.origin.x - px(120.0)
            );
            assert_eq!(window.find("grid-header:0").bounds(), frozen_header);
            assert_eq!(window.find("grid-cell:0:0").bounds(), frozen_cell);
            assert_eq!(
                window.find("grid-scrollable-header-pane").bounds(),
                header_pane
            );
            assert_eq!(window.find("grid-scrollable-row:0").bounds(), row_pane);
        })
        .unwrap();
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
