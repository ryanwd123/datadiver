//! A window for trying the formatting rules on sample data: format sets on
//! the left (each its rules and the columns they apply to), presets to
//! insert, the rules editor with completion, its problems, and the grid.
//!
//! Run with `cargo run -p diver_format --example demo`. Builds only this
//! crate and `code_editor`.
//!
//! `--capture out.png` paints one frame to a PNG and exits; `--set N`
//! selects a set first, `--rules TEXT` replaces its rules, and `--suggest`
//! opens completion at the end of the rules (`--caret N` puts the caret at
//! byte N instead), `--size 1200x400` sets the picture's size, and
//! `--click X,Y` (repeatable, in DIPs) clicks there before the picture.

#![windows_subsystem = "windows"]

mod data;
mod gfx;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use code_editor::{Action, DecorationId, Editor, Event, Options, Range as Sel, TextEdit, Theme};
use diver_format::check::Checked;
use diver_format::draw::{self, Rect};
use diver_format::editor::{FormatLanguage, color_decorations, pos};
use diver_format::style::{Align, CellStyle};
use diver_format::{Column, FormatSet, PRESETS, Severity, TableStats, ValueKind, ValuesRow, check};
use gfx::{Fonts, Gfx, rgb};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

const WM_TEXT_CHANGED: u32 = WM_APP + 1;

const PANEL_W: f32 = 290.0;
const TITLE_H: f32 = 30.0;
const EDITOR_H: f32 = 190.0;
const PROBLEMS_H: f32 = 84.0;
const ROW_H: f32 = 23.0;
const HEADER_H: f32 = 26.0;
const SET_H: f32 = 40.0;
const PRESET_H: f32 = 21.0;

const PANEL_BG: u32 = 0xf3f3f3;
const TEXT: u32 = 0x2b2b2b;
const DIM: u32 = 0x767676;
const ACCENT: u32 = 0x0e639c;
const SELECTED: u32 = 0xd6e6f5;
const HOVER: u32 = 0xe6e6e6;
const LINE: u32 = 0xdcdcdc;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Set(usize),
    NewSet,
    DeleteSet,
    Column(usize),
    Preset(usize),
    Problem(usize),
    Grid,
    Presets,
}

struct App {
    factory: ID2D1Factory,
    fonts: Fonts,
    target: Option<ID2D1HwndRenderTarget>,
    editor: Editor,
    table: data::Table,
    /// The table by row, for running the rules.
    rows: Vec<Vec<diver_format::Value>>,
    sets: Vec<FormatSet>,
    checked: Vec<Checked>,
    selected: usize,
    scroll: usize,
    preset_scroll: usize,
    hits: Vec<(Rect, Hit)>,
    hover: Option<Hit>,
    swatches: Vec<DecorationId>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// Kept apart from the app, so messages that arrive while the app is
    /// busy (the editor placing the IME window sends one) can still reach it.
    static EDITOR: RefCell<Option<Editor>> = const { RefCell::new(None) };
}

fn editor() -> Option<Editor> {
    EDITOR.with(|e| e.borrow().clone())
}

/// Runs `f` on the app, unless a message arrived while the app was already
/// in use: calls into the editor can send the window messages before they
/// return, and those are skipped rather than borrowing the app twice.
fn with_app<T>(f: impl FnOnce(&mut App) -> T) -> Option<T> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Groups and presets as the list shows them.
enum Entry {
    Group(&'static str),
    Preset(usize),
}

fn preset_entries() -> Vec<Entry> {
    let mut out = Vec::new();
    let mut group = "";
    for (i, preset) in PRESETS.iter().enumerate() {
        if preset.group != group {
            group = preset.group;
            out.push(Entry::Group(group));
        }
        out.push(Entry::Preset(i));
    }
    out
}

impl App {
    fn recheck(&mut self, i: usize) {
        let checked = check(&self.sets[i].rules, &self.table.columns);
        self.checked[i] = checked;
    }

    fn select(&mut self, i: usize) {
        self.selected = i.min(self.sets.len().saturating_sub(1));
        let text = self.sets.get(self.selected).map_or(String::new(), |s| s.rules.clone());
        self.editor.set_text(&text);
        self.refresh_swatches();
    }

    fn refresh_swatches(&mut self) {
        let text = self.editor.text();
        self.swatches = self.editor.delta_decorations(&self.swatches, &color_decorations(&text));
    }

    /// Takes the editor's text into the selected set.
    fn sync_text(&mut self) {
        let text = self.editor.text();
        if let Some(set) = self.sets.get_mut(self.selected)
            && set.rules != text
        {
            set.rules = text;
            self.recheck(self.selected);
        }
        self.refresh_swatches();
    }

    fn insert_preset(&mut self, i: usize) {
        if self.sets.is_empty() {
            self.new_set();
        }
        let target = self.sets[self.selected].columns.first().cloned();
        let rule = PRESETS[i].text(&self.table.columns, target.as_deref());
        let text = self.editor.text();
        let trimmed = text.trim_end();
        let insert = if trimmed.is_empty() {
            rule
        } else if trimmed.ends_with(';') {
            format!("\n{rule}")
        } else {
            format!(";\n{rule}")
        };
        let at = pos(&text, trimmed.len());
        let end = pos(&text, text.len());
        self.editor.apply_edits(&[TextEdit { start: at, end, text: insert }]);
        let caret = pos(&self.editor.text(), self.editor.text().len());
        self.editor.set_selections(&[Sel::caret(caret)]);
    }

    fn new_set(&mut self) {
        self.sets.push(FormatSet::default());
        self.checked.push(Checked::default());
        self.select(self.sets.len() - 1);
    }

    fn delete_set(&mut self) {
        if self.sets.is_empty() {
            return;
        }
        self.sets.remove(self.selected);
        self.checked.remove(self.selected);
        self.select(self.selected.saturating_sub(1));
    }

    fn toggle_column(&mut self, i: usize) {
        if self.sets.is_empty() {
            self.new_set();
        }
        let name = self.table.columns[i].name.clone();
        let columns = &mut self.sets[self.selected].columns;
        match columns.iter().position(|c| *c == name) {
            Some(at) => {
                columns.remove(at);
            }
            None => columns.push(name),
        }
    }

    fn click(&mut self, hit: Hit) {
        match hit {
            Hit::Set(i) => self.select(i),
            Hit::NewSet => self.new_set(),
            Hit::DeleteSet => self.delete_set(),
            Hit::Column(i) => self.toggle_column(i),
            Hit::Preset(i) => self.insert_preset(i),
            Hit::Problem(i) => {
                if let Some(d) = self.checked.get(self.selected).and_then(|c| c.diagnostics.get(i)) {
                    let text = self.editor.text();
                    let (a, b) = (pos(&text, d.span.start), pos(&text, d.span.end));
                    self.editor.set_selections(&[Sel { anchor: a, head: b }]);
                }
            }
            Hit::Grid | Hit::Presets => {}
        }
        self.editor.focus();
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
            .map(|(_, h)| *h)
    }

    /// The editor's place, in DIPs.
    fn editor_rect(width: f32) -> Rect {
        Rect::new(PANEL_W + 1.0, TITLE_H, width - PANEL_W - 1.0, EDITOR_H)
    }

    fn style(&self, column: usize, row: usize, stats: &TableStats) -> CellStyle {
        let mut style = CellStyle::default();
        let name = &self.table.columns[column].name;
        let values = ValuesRow { values: &self.rows[row], index: row };
        for (set, checked) in self.sets.iter().zip(&self.checked) {
            if set.columns.iter().any(|c| c == name) {
                checked.program.apply(column, &values, stats, &mut style);
            }
        }
        style
    }

    fn paint(&mut self, g: &Gfx, width: f32, height: f32) {
        self.hits.clear();
        g.fill(Rect::new(0.0, 0.0, width, height), rgb(0xffffff));
        self.paint_panel(g, height);
        self.paint_title(g, width);
        self.editor.paint_into(g.rt);
        let problems = Rect::new(PANEL_W + 1.0, TITLE_H + EDITOR_H, width - PANEL_W - 1.0, PROBLEMS_H);
        self.paint_problems(g, problems);
        let top = problems.bottom();
        self.paint_grid(g, Rect::new(PANEL_W + 1.0, top, width - PANEL_W - 1.0, height - top));
    }

    fn paint_title(&self, g: &Gfx, width: f32) {
        let r = Rect::new(PANEL_W + 1.0, 0.0, width - PANEL_W - 1.0, TITLE_H);
        g.fill(r, rgb(0xfafafa));
        g.fill(Rect::new(r.x, r.bottom() - 1.0, r.w, 1.0), rgb(LINE));
        let columns = self.sets.get(self.selected).map(|s| s.columns.join(", ")).unwrap_or_default();
        let title = if columns.is_empty() { "Rules (pick columns on the left)".to_string() } else { format!("Rules for {columns}") };
        g.text(&title, &g.fonts.ui, Rect::new(r.x + 10.0, r.y, r.w * 0.6, r.h), rgb(TEXT));
        let hint = "Ctrl+Space completes  ·  \" columns  ·  # colors  ·  ; separates rules";
        g.text(hint, &g.fonts.small, Rect::new(r.right() - 420.0, r.y, 410.0, r.h), rgb(DIM));
    }

    fn section(&self, g: &Gfx, title: &str, y: f32) -> f32 {
        g.text(title, &g.fonts.ui_bold, Rect::new(12.0, y, PANEL_W - 24.0, 24.0), rgb(DIM));
        y + 24.0
    }

    fn row_bg(&self, g: &Gfx, r: Rect, hit: Hit, selected: bool) {
        if selected {
            g.fill(r, rgb(SELECTED));
        } else if self.hover == Some(hit) {
            g.fill(r, rgb(HOVER));
        }
    }

    fn paint_panel(&mut self, g: &Gfx, height: f32) {
        g.fill(Rect::new(0.0, 0.0, PANEL_W, height), rgb(PANEL_BG));
        g.fill(Rect::new(PANEL_W, 0.0, 1.0, height), rgb(LINE));
        let mut y = self.section(g, "FORMAT SETS", 4.0);
        for (i, set) in self.sets.iter().enumerate() {
            let r = Rect::new(6.0, y, PANEL_W - 12.0, SET_H - 2.0);
            self.row_bg(g, r, Hit::Set(i), i == self.selected);
            let summary = set
                .rules
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with("--"))
                .unwrap_or("(no rules)");
            let errors = self.checked[i].errors().count();
            let color = if errors > 0 { rgb(0xc00000) } else { rgb(TEXT) };
            g.text(summary, &g.fonts.small, Rect::new(r.x + 8.0, r.y + 2.0, r.w - 14.0, 18.0), color);
            let columns = if set.columns.is_empty() { "no columns".to_string() } else { set.columns.join(", ") };
            let columns = if errors > 0 { format!("{columns}  ·  {errors} problem{}", if errors == 1 { "" } else { "s" }) } else { columns };
            g.text(&columns, &g.fonts.small, Rect::new(r.x + 8.0, r.y + 19.0, r.w - 14.0, 16.0), rgb(DIM));
            self.hits.push((r, Hit::Set(i)));
            y += SET_H;
        }
        let new = Rect::new(10.0, y + 2.0, 96.0, 22.0);
        let delete = Rect::new(112.0, y + 2.0, 96.0, 22.0);
        for (r, label, hit) in [(new, "+ New set", Hit::NewSet), (delete, "Delete set", Hit::DeleteSet)] {
            g.fill_rounded(r, 3.0, if self.hover == Some(hit) { rgb(0xdedede) } else { rgb(0xe9e9e9) });
            g.text(label, &g.fonts.small, Rect::new(r.x + 10.0, r.y, r.w - 10.0, r.h), rgb(TEXT));
            self.hits.push((r, hit));
        }
        y += 34.0;
        y = self.section(g, "APPLY TO COLUMNS", y);
        let applied: Vec<String> = self.sets.get(self.selected).map(|s| s.columns.clone()).unwrap_or_default();
        let per_row = 3;
        let cell_w = (PANEL_W - 16.0) / per_row as f32;
        for (i, column) in self.table.columns.iter().enumerate() {
            let r = Rect::new(8.0 + (i % per_row) as f32 * cell_w, y + (i / per_row) as f32 * 22.0, cell_w - 4.0, 21.0);
            self.row_bg(g, r, Hit::Column(i), false);
            let on = applied.contains(&column.name);
            let bx = Rect::new(r.x + 4.0, r.y + 5.0, 11.0, 11.0);
            if on {
                g.fill(bx, rgb(ACCENT));
                g.mark(&draw::Mark::Polyline {
                    points: vec![(bx.x + 2.5, bx.y + 5.5), (bx.x + 4.5, bx.y + 8.0), (bx.x + 8.5, bx.y + 3.0)],
                    color: rgb(0xffffff),
                    width: 1.6,
                });
            } else {
                g.fill(bx, rgb(0xffffff));
                g.outline(bx, rgb(0x9a9a9a), 1.0);
            }
            g.text(&column.name, &g.fonts.small, Rect::new(r.x + 20.0, r.y, r.w - 20.0, r.h), rgb(TEXT));
            self.hits.push((r, Hit::Column(i)));
        }
        y += self.table.columns.len().div_ceil(per_row) as f32 * 22.0 + 8.0;
        y = self.section(g, "PRESETS  (click to add)", y);
        let footer = 46.0;
        let area = Rect::new(0.0, y, PANEL_W, height - y - footer);
        self.hits.push((area, Hit::Presets));
        let entries = preset_entries();
        let visible = (area.h / PRESET_H).floor().max(0.0) as usize;
        self.preset_scroll = self.preset_scroll.min(entries.len().saturating_sub(visible));
        g.clip(area);
        for (n, entry) in entries.iter().skip(self.preset_scroll).take(visible + 1).enumerate() {
            let r = Rect::new(6.0, area.y + n as f32 * PRESET_H, PANEL_W - 12.0, PRESET_H);
            match entry {
                Entry::Group(name) => {
                    g.text(name, &g.fonts.ui_bold, Rect::new(r.x + 6.0, r.y + 2.0, r.w, r.h), rgb(ACCENT));
                }
                Entry::Preset(i) => {
                    self.row_bg(g, r, Hit::Preset(*i), false);
                    g.text(PRESETS[*i].name, &g.fonts.small, Rect::new(r.x + 16.0, r.y, r.w - 16.0, r.h), rgb(TEXT));
                    self.hits.push((r, Hit::Preset(*i)));
                }
            }
        }
        g.unclip();
        let foot = Rect::new(0.0, height - footer, PANEL_W, footer);
        g.fill(Rect::new(0.0, foot.y, PANEL_W, 1.0), rgb(LINE));
        if let Some(Hit::Preset(i)) = self.hover {
            let preset = &PRESETS[i];
            g.text(preset.description, &g.fonts.small, Rect::new(10.0, foot.y + 3.0, PANEL_W - 20.0, 18.0), rgb(TEXT));
            let target = self.sets.get(self.selected).and_then(|s| s.columns.first().cloned());
            let text = preset.text(&self.table.columns, target.as_deref());
            g.text(&text, &g.fonts.small, Rect::new(10.0, foot.y + 22.0, PANEL_W - 20.0, 18.0), rgb(DIM));
        } else {
            g.text("Hover a preset to see its rule.", &g.fonts.small, Rect::new(10.0, foot.y + 3.0, PANEL_W - 20.0, 18.0), rgb(DIM));
        }
    }

    fn paint_problems(&mut self, g: &Gfx, r: Rect) {
        g.fill(r, rgb(0xfbfbfb));
        g.fill(Rect::new(r.x, r.y, r.w, 1.0), rgb(LINE));
        g.fill(Rect::new(r.x, r.bottom() - 1.0, r.w, 1.0), rgb(LINE));
        let Some(checked) = self.checked.get(self.selected) else { return };
        let text = &self.sets[self.selected].rules;
        if checked.diagnostics.is_empty() {
            let rules = checked.program.rules.len();
            let msg = if text.trim().is_empty() {
                "No rules yet: type one, or click a preset.".to_string()
            } else {
                format!("No problems  ·  {rules} rule{} running", if rules == 1 { "" } else { "s" })
            };
            g.text(&msg, &g.fonts.small, Rect::new(r.x + 12.0, r.y + 4.0, r.w - 24.0, 18.0), rgb(0x2e7d32));
            return;
        }
        for (i, d) in checked.diagnostics.iter().take(4).enumerate() {
            let line = Rect::new(r.x + 4.0, r.y + 4.0 + i as f32 * 19.0, r.w - 8.0, 19.0);
            self.row_bg(g, line, Hit::Problem(i), false);
            let p = pos(text, d.span.start);
            let (label, color) = match d.severity {
                Severity::Error => ("error", rgb(0xc00000)),
                Severity::Warning => ("warning", rgb(0xb36b00)),
            };
            g.text(label, &g.fonts.ui_bold, Rect::new(line.x + 8.0, line.y, 60.0, line.h), color);
            let at = format!("Ln {}, Col {}", p.line + 1, p.col + 1);
            g.text(&at, &g.fonts.small, Rect::new(line.x + 66.0, line.y, 80.0, line.h), rgb(DIM));
            g.text(&d.message, &g.fonts.small, Rect::new(line.x + 146.0, line.y, line.w - 150.0, line.h), rgb(TEXT));
            self.hits.push((line, Hit::Problem(i)));
        }
    }

    fn paint_grid(&mut self, g: &Gfx, area: Rect) {
        self.hits.push((area, Hit::Grid));
        let stats = TableStats::new(&self.table.values);
        let applied: Vec<String> = self.sets.get(self.selected).map(|s| s.columns.clone()).unwrap_or_default();
        let formatted: Vec<bool> = self
            .table
            .columns
            .iter()
            .map(|c| self.sets.iter().any(|s| s.columns.contains(&c.name)))
            .collect();
        g.clip(area);
        let header = Rect::new(area.x, area.y, area.w, HEADER_H);
        g.fill(header, rgb(0xf0f0f0));
        let mut x = area.x;
        for (i, column) in self.table.columns.iter().enumerate() {
            let w = self.table.widths[i];
            let cell = Rect::new(x, header.y, w, HEADER_H);
            let selected = applied.contains(&column.name);
            if selected {
                g.fill(Rect::new(cell.x, cell.bottom() - 3.0, cell.w, 3.0), rgb(ACCENT));
            }
            let font = if selected { &g.fonts.ui_bold } else { &g.fonts.small };
            g.text(&column.name, font, Rect::new(cell.x + 6.0, cell.y, cell.w - 16.0, cell.h), rgb(TEXT));
            if formatted[i] {
                g.mark(&draw::Mark::Circle { center: (cell.right() - 8.0, cell.y + cell.h / 2.0), radius: 2.5, color: rgb(ACCENT) });
            }
            g.fill(Rect::new(cell.right() - 1.0, cell.y, 1.0, cell.h), rgb(LINE));
            self.hits.push((cell, Hit::Column(i)));
            x += w;
        }
        g.fill(Rect::new(area.x, header.bottom() - 1.0, area.w, 1.0), rgb(LINE));
        let visible = ((area.h - HEADER_H) / ROW_H).ceil() as usize;
        self.scroll = self.scroll.min(self.table.rows().saturating_sub(visible.saturating_sub(1)));
        for (n, row) in (self.scroll..self.table.rows()).take(visible).enumerate() {
            let y = header.bottom() + n as f32 * ROW_H;
            let mut x = area.x;
            for (i, column) in self.table.columns.iter().enumerate() {
                let w = self.table.widths[i];
                let cell = Rect::new(x, y, w, ROW_H);
                g.fill(cell, if row % 2 == 1 { rgb(0xfafafa) } else { rgb(0xffffff) });
                let style = self.style(i, row, &stats);
                let layout = draw::layout(&style, cell);
                g.clip(cell);
                for mark in &layout.under {
                    g.mark(mark);
                }
                if let Some(text_rect) = layout.text {
                    let default_align = match column.kind {
                        ValueKind::Number => Align::Right,
                        _ => Align::Left,
                    };
                    let color = style.text.unwrap_or(rgb(TEXT));
                    g.cell_text(&self.table.text[i][row], text_rect, &style, style.align.unwrap_or(default_align), color);
                }
                for mark in &layout.over {
                    g.mark(mark);
                }
                g.unclip();
                g.fill(Rect::new(cell.right() - 1.0, cell.y, 1.0, cell.h), rgb(0xececec));
                g.fill(Rect::new(cell.x, cell.bottom() - 1.0, cell.w, 1.0), rgb(0xececec));
                x += w;
            }
        }
        g.unclip();
    }
}

fn paint_window(hwnd: HWND) {
    with_app(|app| unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let dpi = GetDpiForWindow(hwnd) as f32;
        if app.target.is_none() {
            let props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U { width: rc.right as u32, height: rc.bottom as u32 },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            app.target = app.factory.CreateHwndRenderTarget(&Default::default(), &props).ok();
        }
        let Some(target) = app.target.clone() else { return };
        target.SetDpi(dpi, dpi);
        let _ = target.Resize(&D2D_SIZE_U { width: rc.right as u32, height: rc.bottom as u32 });
        target.BeginDraw();
        let factory = app.factory.clone();
        let fonts = std::ptr::addr_of!(app.fonts);
        if let Ok(g) = Gfx::new(&target, &factory, &*fonts) {
            let scale = dpi / 96.0;
            app.paint(&g, rc.right as f32 / scale, rc.bottom as f32 / scale);
        }
        if target.EndDraw(None, None).is_err() {
            app.target = None;
        }
    });
}

fn layout(hwnd: HWND) {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
    if let Some(ed) = editor() {
        let r = App::editor_rect(rc.right as f32 / scale);
        ed.set_bounds((r.x * scale) as i32, (r.y * scale) as i32, (r.w * scale) as i32, (r.h * scale) as i32);
    }
}

fn dips(hwnd: HWND, lparam: LPARAM) -> (f32, f32) {
    let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
    let x = (lparam.0 & 0xffff) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
    (x / scale, y / scale)
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if let Some(ed) = editor()
            && let Some(r) = ed.handle_message(msg, wparam, lparam)
        {
            return r;
        }
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);
                paint_window(hwnd);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                layout(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = dips(hwnd, lparam);
                with_app(|app| {
                    if let Some(hit) = app.hit(x, y) {
                        app.click(hit);
                    }
                });
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = dips(hwnd, lparam);
                let changed = with_app(|app| {
                    let hover = app.hit(x, y);
                    std::mem::replace(&mut app.hover, hover) != hover
                });
                if changed == Some(true) {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_MOUSEWHEEL => {
                let mut p = POINT { x: (lparam.0 & 0xffff) as i16 as i32, y: ((lparam.0 >> 16) & 0xffff) as i16 as i32 };
                let _ = ScreenToClient(hwnd, &mut p);
                let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
                let delta = ((wparam.0 >> 16) & 0xffff) as i16 as i32;
                let steps = -(delta / 40);
                with_app(|app| {
                    let shift = |v: usize| (v as i32 + steps).max(0) as usize;
                    match app.hit(p.x as f32 / scale, p.y as f32 / scale) {
                        Some(Hit::Grid) => app.scroll = shift(app.scroll),
                        Some(Hit::Presets | Hit::Preset(_)) => app.preset_scroll = shift(app.preset_scroll),
                        _ => {}
                    }
                });
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_TEXT_CHANGED => {
                with_app(App::sync_text);
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_GETDLGCODE => LRESULT(DLGC_WANTALLKEYS as isize),
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

struct Args {
    capture: Option<PathBuf>,
    set: Option<usize>,
    rules: Option<String>,
    suggest: bool,
    caret: Option<usize>,
    size: Option<(u32, u32)>,
    clicks: Vec<(f32, f32)>,
}

fn args() -> Args {
    let mut out = Args { capture: None, set: None, rules: None, suggest: false, caret: None, size: None, clicks: Vec::new() };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--capture" => out.capture = it.next().map(PathBuf::from),
            "--set" => out.set = it.next().and_then(|n| n.parse().ok()),
            "--rules" => out.rules = it.next(),
            "--suggest" => out.suggest = true,
            "--caret" => out.caret = it.next().and_then(|n| n.parse().ok()),
            "--click" => {
                if let Some((x, y)) = it.next().as_deref().and_then(|s| s.split_once(','))
                    && let (Ok(x), Ok(y)) = (x.parse(), y.parse())
                {
                    out.clicks.push((x, y));
                }
            }
            "--size" => {
                out.size = it.next().and_then(|s| {
                    let (w, h) = s.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                })
            }
            _ => {}
        }
    }
    out
}

/// Paints a frame at 96 DPI, and saves it when given a path.
fn capture(path: Option<&std::path::Path>, width: u32, height: u32) -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let bitmap = wic.CreateBitmap(width, height, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        with_app(|app| -> Result<()> {
            let r = App::editor_rect(width as f32);
            app.editor.set_bounds(r.x as i32, r.y as i32, r.w as i32, r.h as i32);
            let target = app.factory.CreateWicBitmapRenderTarget(&bitmap, &props)?;
            target.BeginDraw();
            let factory = app.factory.clone();
            let fonts = std::ptr::addr_of!(app.fonts);
            let g = Gfx::new(&target, &factory, &*fonts)?;
            app.paint(&g, width as f32, height as f32);
            target.EndDraw(None, None)
        })
        .unwrap_or(Ok(()))?;
        let Some(path) = path else { return Ok(()) };
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), 0x4000_0000)?;
        let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.ok_or_else(Error::empty)?;
        frame.Initialize(None)?;
        frame.SetSize(width, height)?;
        let mut format = GUID_WICPixelFormat32bppPBGRA;
        frame.SetPixelFormat(&mut format)?;
        frame.WriteSource(&bitmap, std::ptr::null())?;
        frame.Commit()?;
        encoder.Commit()?;
    }
    Ok(())
}

/// A windowed app has no console, so a panic would just close it: say what
/// happened instead, and keep it in a file too.
fn report_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let text = format!("{info}

{}", std::backtrace::Backtrace::force_capture());
        let path = std::env::temp_dir().join("diver_format_demo_panic.txt");
        let _ = std::fs::write(&path, &text);
        default(info);
        let message = HSTRING::from(format!("{info}

Details: {}", path.display()));
        unsafe {
            MessageBoxW(None, &message, w!("Demo panicked"), MB_ICONERROR);
        }
    }));
}

fn main() -> Result<()> {
    report_panics();
    let args = args();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSW {
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hInstance: instance,
            lpszClassName: w!("DiverFormatDemo"),
            lpfnWndProc: Some(wndproc),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let (width, height) = (1500, 940);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("DiverFormatDemo"),
            w!("Conditional formatting rules"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            width,
            height,
            None,
            None,
            Some(instance),
            None,
        )?;
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let options = Options {
            font_size: 13.5,
            sticky_scroll: false,
            folding: false,
            occurrences_highlight: false,
            ..Options::default()
        };
        let ed = Editor::create_hosted(hwnd, options, Theme::Light)?;
        // The SQL grammar brings `--` comments, auto-closed quotes and
        // brackets; the rules' own service colors the text.
        ed.set_language("sql");
        let table = data::sample(60);
        let columns: Rc<RefCell<Vec<Column>>> = Rc::new(RefCell::new(table.columns.clone()));
        let language = FormatLanguage::new(columns);
        ed.set_language_service(Some(Rc::new(language.clone())));
        ed.set_completion_provider(language);
        ed.on_event(move |_, e| {
            if let Event::ContentChanged { .. } = e {
                let _ = PostMessageW(Some(hwnd), WM_TEXT_CHANGED, WPARAM(0), LPARAM(0));
            }
        });
        let rows = (0..table.rows()).map(|r| table.values.iter().map(|c| c[r].clone()).collect()).collect();
        let sets = data::starting_sets();
        let mut app = App {
            factory,
            fonts: gfx::Fonts::new()?,
            target: None,
            editor: ed.clone(),
            table,
            rows,
            checked: vec![Checked::default(); sets.len()],
            sets,
            selected: 0,
            scroll: 0,
            preset_scroll: 0,
            hits: Vec::new(),
            hover: None,
            swatches: Vec::new(),
        };
        for i in 0..app.sets.len() {
            app.recheck(i);
        }
        app.select(args.set.unwrap_or(0));
        if let Some(rules) = &args.rules {
            app.editor.set_text(rules);
            app.sync_text();
        }
        APP.with(|a| *a.borrow_mut() = Some(app));
        EDITOR.with(|e| *e.borrow_mut() = Some(ed.clone()));
        layout(hwnd);

        if let Some(path) = &args.capture {
            if args.suggest || args.caret.is_some() {
                let text = ed.text();
                let caret = pos(&text, args.caret.unwrap_or(text.len()));
                ed.set_selections(&[Sel::caret(caret)]);
                ed.focus();
                if args.suggest {
                    ed.run(Action::TriggerSuggest);
                }
            }
            let (w, h) = args.size.unwrap_or((width as u32, height as u32));
            // Clicks go through the window procedure like real ones, at the
            // places the first frame laid things out.
            if !args.clicks.is_empty() {
                capture(None, w, h)?;
                let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
                for (x, y) in &args.clicks {
                    let l = LPARAM(((y * scale) as isize) << 16 | (x * scale) as isize);
                    SendMessageW(hwnd, WM_MOUSEMOVE, Some(WPARAM(0)), Some(l));
                    SendMessageW(hwnd, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(l));
                    SendMessageW(hwnd, WM_LBUTTONUP, Some(WPARAM(0)), Some(l));
                    let mut msg = MSG::default();
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
                        DispatchMessageW(&msg);
                    }
                    capture(None, w, h)?;
                }
            }
            let result = capture(Some(path), w, h);
            EDITOR.with(|e| e.borrow_mut().take());
            APP.with(|a| a.borrow_mut().take());
            return result;
        }

        let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);
        ed.focus();
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        EDITOR.with(|e| e.borrow_mut().take());
        APP.with(|a| a.borrow_mut().take());
        Ok(())
    }
}
