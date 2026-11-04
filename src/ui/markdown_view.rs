//! A scrolling Markdown page, such as a help page: laid out once per width
//! and font, with links to follow and headings to jump to.

use std::ops::Range;
use std::rc::Rc;

use code_editor::markdown::{Block, Inline, Style, parse};

use crate::ui::hit::Hit;
use crate::ui::platform::text::{RunStyle, styled_layout};
use crate::ui::platform::{Canvas, Color, Cursor, Font, Point, Rect, TextLayout, TextOptions, point, rect, text};
use crate::ui::widgets::{Fonts, colors, scroll, sql_highlight};

/// Lines longer than this are hard to read, so wider pages keep a margin.
const MAX_TEXT_WIDTH: f32 = 880.0;
const PADDING: f32 = 28.0;

pub struct MarkdownView {
    blocks: Vec<Block>,
    laid: Option<Laid>,
    pub scroll: scroll::Scroll,
    viewport: Rect,
    /// A heading to scroll to once the page is laid out.
    pending_anchor: Option<String>,
}

/// The page laid out for one width and font.
struct Laid {
    width: f32,
    font: Font,
    items: Vec<Item>,
    height: f32,
    /// Where each link is, in content coordinates, and where it goes.
    links: Vec<(Rect, String)>,
    /// Each heading's anchor and its top.
    anchors: Vec<(String, f32)>,
}

enum Item {
    Text {
        at: Point,
        layout: Rc<TextLayout>,
        color: Color,
        runs: Vec<(usize, usize, Color)>,
        /// Backgrounds behind code spans, relative to `at`.
        code: Vec<Rect>,
    },
    Fill {
        r: Rect,
        color: Color,
        radius: f32,
    },
}

/// What a heading is linked as: lowercase words joined by dashes.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if (ch == ' ' || ch == '-' || ch == '_') && !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Inline text as a layout's styles, colors, code ranges and links.
struct Styled {
    runs: Vec<(Range<usize>, RunStyle)>,
    colors: Vec<(usize, usize, Color)>,
    code: Vec<Range<usize>>,
    links: Vec<(Range<usize>, String)>,
}

fn styled(inline: &Inline, code_font: &Font) -> Styled {
    let mut out = Styled { runs: Vec::new(), colors: Vec::new(), code: Vec::new(), links: inline.links.clone() };
    for (range, style) in &inline.spans {
        match style {
            Style::Bold => out.runs.push((range.clone(), RunStyle::Semibold)),
            Style::Italic => out.runs.push((range.clone(), RunStyle::Italic)),
            Style::Code => {
                out.runs.push((range.clone(), RunStyle::Font(code_font.clone())));
                out.code.push(range.clone());
            }
            Style::Link => {
                out.runs.push((range.clone(), RunStyle::Underline));
                out.colors.push((range.start, range.end, colors::FOCUS));
            }
        }
    }
    out
}

impl MarkdownView {
    pub fn new(markdown: &str) -> Self {
        Self {
            blocks: parse(markdown),
            laid: None,
            scroll: scroll::Scroll::default(),
            viewport: Rect::default(),
            pending_anchor: None,
        }
    }

    /// The headings' anchors, in page order.
    pub fn anchors(&self) -> Vec<String> {
        self.blocks
            .iter()
            .filter_map(|block| match block {
                Block::Heading(_, text) => Some(slug(&text.text)),
                _ => None,
            })
            .collect()
    }

    /// The page's text, without its markup.
    pub fn plain_text(&self) -> String {
        let mut out = Vec::new();
        for block in &self.blocks {
            match block {
                Block::Heading(_, text) | Block::Paragraph(text) | Block::Quote(text) | Block::Item { text, .. } => {
                    out.push(text.text.clone())
                }
                Block::Code { text, .. } => out.push(text.clone()),
                Block::Table { header, rows } => {
                    for row in std::iter::once(header).chain(rows) {
                        out.push(row.iter().map(|cell| cell.text.as_str()).collect::<Vec<_>>().join(" | "));
                    }
                }
                Block::Rule => {}
            }
        }
        out.join("\n")
    }

    /// Scrolls to the heading with `anchor`, now or once the page is laid out.
    pub fn scroll_to(&mut self, anchor: &str) {
        match self.laid.as_ref().and_then(|laid| laid.anchors.iter().find(|(a, _)| a == anchor)) {
            Some((_, top)) => self.scroll.offset = (top - 12.0).max(0.0),
            None if self.laid.is_none() => self.pending_anchor = Some(anchor.to_string()),
            None => {}
        }
    }

    pub fn scroll_to_top(&mut self) {
        self.pending_anchor = None;
        self.scroll.offset = 0.0;
    }

    fn content_height(&self) -> f32 {
        self.laid.as_ref().map_or(0.0, |laid| laid.height)
    }

    pub fn wheel(&mut self, delta: Point, line: f32) -> bool {
        let height = self.content_height();
        self.scroll.scroll_by(-delta.y * line * 3.0, self.viewport.h, height)
    }

    pub fn key(&mut self, key: &str, line: f32) -> bool {
        let page = (self.viewport.h - line * 2.0).max(line);
        let height = self.content_height();
        let delta = match key {
            "down" => line * 2.0,
            "up" => -line * 2.0,
            "pagedown" | "space" => page,
            "pageup" => -page,
            "home" => -height,
            "end" => height,
            _ => return false,
        };
        self.scroll.scroll_by(delta, self.viewport.h, height);
        true
    }

    fn bar(&self) -> Rect {
        rect(self.viewport.right() - scroll::BAR_WIDTH, self.viewport.y, scroll::BAR_WIDTH, self.viewport.h)
    }

    pub fn press_bar(&mut self, pos: Point) {
        let bar = self.bar();
        let height = self.content_height();
        self.scroll.press(pos.y - bar.y, bar.h, self.viewport.h, height);
    }

    pub fn drag(&mut self, pos: Point) {
        let bar = self.bar();
        let height = self.content_height();
        self.scroll.drag(pos.y - bar.y, bar.h, self.viewport.h, height);
    }

    pub fn release(&mut self) {
        self.scroll.release();
    }

    pub fn dragging(&self) -> bool {
        self.scroll.dragging()
    }

    /// Where link `index` goes.
    pub fn link(&self, index: usize) -> Option<&str> {
        self.laid.as_ref()?.links.get(index).map(|(_, target)| target.as_str())
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts) {
        self.viewport = r;
        let width = (r.w - PADDING * 2.0 - scroll::BAR_WIDTH).clamp(120.0, MAX_TEXT_WIDTH);
        let font = fonts.ui.sized(fonts.ui.size + 1.0);
        if self.laid.as_ref().is_none_or(|laid| laid.width != width || laid.font != font) {
            self.laid = Some(layout(&self.blocks, width, &font, &fonts.mono));
        }
        if let Some(anchor) = self.pending_anchor.take() {
            self.scroll_to(&anchor);
        }
        let Some(laid) = &self.laid else { return };
        self.scroll.clamp(r.h, laid.height);
        let origin = point(r.x + PADDING, r.y - self.scroll.offset);
        canvas.push_clip(r);
        canvas.hit(r, Hit::DialogScroll);
        for item in &laid.items {
            match item {
                Item::Text { at, layout, color, runs, code } => {
                    let top = origin.y + at.y;
                    if top > r.bottom() || top + layout.height < r.y {
                        continue;
                    }
                    let at = point(origin.x + at.x, top);
                    for back in code {
                        canvas.fill_rounded(back.offset(at.x, at.y), 3.0, colors::KEY_BACKGROUND);
                    }
                    canvas.draw_layout_runs(layout, at, *color, runs);
                }
                Item::Fill { r: fill, color, radius } => {
                    let fill = fill.offset(origin.x, origin.y);
                    if fill.y > r.bottom() || fill.bottom() < r.y {
                        continue;
                    }
                    if *radius > 0.0 {
                        canvas.fill_rounded(fill, *radius, *color);
                    } else {
                        canvas.fill(fill, *color);
                    }
                }
            }
        }
        for (index, (link, target)) in laid.links.iter().enumerate() {
            canvas.hit_with(link.offset(origin.x, origin.y), Hit::HelpLink(index), Cursor::Hand, Some(target));
        }
        canvas.pop_clip();
        let height = laid.height;
        scroll::vertical(canvas, self.bar(), r.h, height, &self.scroll, Hit::DialogBar);
    }
}

/// Lays out `blocks` in a column `width` wide, from the top.
fn layout(blocks: &[Block], width: f32, font: &Font, mono: &Font) -> Laid {
    let line = text::line_height(font);
    let code_font = mono.sized((font.size - 1.0).max(8.0));
    let mut laid = Laid {
        width,
        font: font.clone(),
        items: Vec::new(),
        height: 0.0,
        links: Vec::new(),
        anchors: Vec::new(),
    };
    let gap = (line * 0.75).round();
    let mut y = PADDING * 0.75;
    let mut previous_item = false;
    for (index, block) in blocks.iter().enumerate() {
        let is_item = matches!(block, Block::Item { .. });
        if index > 0 {
            y += match block {
                Block::Heading(level, _) => gap + if *level <= 2 { line } else { line * 0.5 },
                _ if is_item && previous_item => line * 0.25,
                _ => gap,
            };
        }
        previous_item = is_item;
        match block {
            Block::Heading(level, inline) => {
                let size = match level {
                    1 => font.size * 1.75,
                    2 => font.size * 1.35,
                    3 => font.size * 1.12,
                    _ => font.size,
                };
                let heading = font.sized(size.round()).semibold();
                laid.anchors.push((slug(&inline.text), y));
                y += text_item(&mut laid, inline, &heading, &code_font, point(0.0, y), width, colors::FOREGROUND);
                if *level <= 2 {
                    y += 6.0;
                    laid.items.push(Item::Fill { r: rect(0.0, y, width, 1.0), color: colors::BORDER, radius: 0.0 });
                    y += 1.0;
                }
            }
            Block::Paragraph(inline) => {
                y += text_item(&mut laid, inline, font, &code_font, point(0.0, y), width, colors::FOREGROUND);
            }
            Block::Quote(inline) => {
                let start = y;
                let height = text_item(&mut laid, inline, font, &code_font, point(16.0, y + 6.0), width - 28.0, colors::FOREGROUND);
                y += height + 12.0;
                laid.items.insert(
                    laid.items.len() - 1,
                    Item::Fill { r: rect(0.0, start, width, y - start), color: colors::STRIPE, radius: 4.0 },
                );
                laid.items.insert(
                    laid.items.len() - 1,
                    Item::Fill { r: rect(0.0, start, 3.0, y - start), color: colors::FOCUS, radius: 0.0 },
                );
            }
            Block::Item { depth, number, text: inline } => {
                let indent = 4.0 + *depth as f32 * 22.0;
                let marker = number.map_or("\u{2022}".to_string(), |n| format!("{n}."));
                let marker_width = if number.is_some() { 24.0 } else { 16.0 };
                let marker_layout = text::layout(&marker, font, TextOptions::default());
                laid.items.push(Item::Text {
                    at: point(indent, y),
                    layout: marker_layout,
                    color: colors::MUTED,
                    runs: Vec::new(),
                    code: Vec::new(),
                });
                let x = indent + marker_width;
                y += text_item(&mut laid, inline, font, &code_font, point(x, y), width - x, colors::FOREGROUND);
            }
            Block::Code { lang, text: code } => {
                let layout = text::layout(code, &code_font, TextOptions::wrapped(width - 28.0));
                let runs = if lang.is_empty() || lang.eq_ignore_ascii_case("sql") { sql_highlight::runs(code) } else { Vec::new() };
                let height = layout.height + 20.0;
                laid.items.push(Item::Fill { r: rect(0.0, y, width, height), color: colors::STRIPE, radius: 5.0 });
                laid.items.push(Item::Text { at: point(14.0, y + 10.0), layout, color: colors::FOREGROUND, runs, code: Vec::new() });
                y += height;
            }
            Block::Rule => {
                y += 4.0;
                laid.items.push(Item::Fill { r: rect(0.0, y, width, 1.0), color: colors::BORDER, radius: 0.0 });
                y += 5.0;
            }
            Block::Table { header, rows } => {
                y += table(&mut laid, header, rows, font, &code_font, y, width);
            }
        }
    }
    laid.height = y + PADDING;
    laid
}

/// Lays out styled inline text at `at`, returning its height.
fn text_item(laid: &mut Laid, inline: &Inline, font: &Font, code_font: &Font, at: Point, width: f32, color: Color) -> f32 {
    let style = styled(inline, code_font);
    let layout = styled_layout(&inline.text, font, TextOptions::wrapped(width.max(20.0)), &style.runs);
    let code = style
        .code
        .iter()
        .flat_map(|range| layout.range_rects(range.start, range.end))
        .map(|r| rect(r.x - 3.0, r.y + 1.0, r.w + 6.0, r.h - 2.0))
        .collect();
    for (range, target) in style.links {
        for r in layout.range_rects(range.start, range.end) {
            laid.links.push((r.offset(at.x, at.y), target.clone()));
        }
    }
    let height = layout.height;
    laid.items.push(Item::Text { at, layout, color, runs: style.colors, code });
    height
}

/// Lays out a table at `top`: columns as wide as their text, shrunk to fit
/// `width` with the widest wrapping first. Returns its height.
fn table(laid: &mut Laid, header: &[Inline], rows: &[Vec<Inline>], font: &Font, code_font: &Font, top: f32, width: f32) -> f32 {
    const CELL: f32 = 10.0;
    let columns = header.len().max(rows.iter().map(Vec::len).max().unwrap_or(0)).max(1);
    let all = std::iter::once(header).chain(rows.iter().map(Vec::as_slice));
    let mut natural = vec![0.0_f32; columns];
    for row in all.clone() {
        for (index, cell) in row.iter().enumerate() {
            let style = styled(cell, code_font);
            let layout = styled_layout(&cell.text, &font.semibold(), TextOptions::default(), &style.runs);
            natural[index] = natural[index].max(layout.width + CELL * 2.0);
        }
    }
    // Columns over an even share give up width until the table fits.
    let mut widths = natural.clone();
    let mut over = widths.iter().sum::<f32>() - width;
    while over > 0.5 {
        let fair = width / columns as f32;
        let wide: Vec<usize> = (0..columns).filter(|&i| widths[i] > fair).collect();
        if wide.is_empty() {
            break;
        }
        let shrink = over / wide.len() as f32;
        for &i in &wide {
            let next = (widths[i] - shrink).max(fair);
            over -= widths[i] - next;
            widths[i] = next;
        }
        if wide.iter().all(|&i| widths[i] <= fair + 0.5) {
            break;
        }
    }
    let total: f32 = widths.iter().sum();
    let mut y = top;
    for (row_index, row) in all.enumerate() {
        let cell_font = if row_index == 0 { font.semibold() } else { font.clone() };
        let start = laid.items.len();
        let mut x = 0.0;
        let mut height: f32 = 0.0;
        for (index, cell) in row.iter().enumerate().take(columns) {
            let h = text_item(laid, cell, &cell_font, code_font, point(x + CELL, y + 6.0), widths[index] - CELL * 2.0, colors::FOREGROUND);
            height = height.max(h);
            x += widths[index];
        }
        let row_height = height + 12.0;
        let background = if row_index == 0 {
            Some(colors::STRIPE)
        } else {
            None
        };
        if let Some(color) = background {
            laid.items.insert(start, Item::Fill { r: rect(0.0, y, total, row_height), color, radius: 0.0 });
        }
        y += row_height;
        laid.items.push(Item::Fill { r: rect(0.0, y - 1.0, total, 1.0), color: colors::BORDER, radius: 0.0 });
    }
    y - top
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_link_headings() {
        assert_eq!(slug("Ask AI for SQL"), "ask-ai-for-sql");
        assert_eq!(slug("Group By, Pivot By & totals"), "group-by-pivot-by-totals");
        assert_eq!(slug("  @ mentions "), "mentions");
    }
}
