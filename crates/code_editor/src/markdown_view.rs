//! Markdown drawn in the editor's popups (hover text and signature
//! documentation), in the popup font: bold, italic, code and links inline,
//! and paragraphs, lists, quotes, rules and syntax-colored code blocks.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;
use windows::Win32::Graphics::DirectWrite::*;

use crate::markdown::{Block, Inline, Style, parse};
use crate::render::Renderer;
use crate::text::col_to_utf16;
use crate::theme::Ink;

/// Colors for a code block's text, as byte ranges.
pub(crate) type Highlight<'a> = &'a dyn Fn(&str) -> Vec<(Range<usize>, Ink)>;

/// Markdown laid out for one width, ready to draw at any position.
pub(crate) struct MdLayout {
    items: Vec<Item>,
    /// The width the content uses, at most the width it was laid out for.
    pub w: f32,
    pub h: f32,
}

enum Item {
    Text {
        x: f32,
        y: f32,
        layout: IDWriteTextLayout,
        ink: Ink,
        /// UTF-16 ranges drawn in another ink.
        colors: Vec<(u32, u32, Ink)>,
        /// Backgrounds behind code spans, relative to the text.
        code: Vec<[f32; 4]>,
    },
    /// A band across the whole content width, like a code block's background.
    Band { t: f32, b: f32, ink: Ink },
    Fill { l: f32, t: f32, r: f32, b: f32, ink: Ink },
    Rule { y: f32 },
}

fn range(a: u32, b: u32) -> DWRITE_TEXT_RANGE {
    DWRITE_TEXT_RANGE {
        startPosition: a,
        length: b.saturating_sub(a),
    }
}

impl Renderer {
    /// `md` laid out no wider than `max_w` and cut at `max_h`, from the cache
    /// when the same text was laid out at the same size before.
    pub(crate) fn markdown(&self, md: &str, max_w: f32, max_h: f32, highlight: Highlight) -> Option<Rc<MdLayout>> {
        let mut h = DefaultHasher::new();
        md.hash(&mut h);
        max_w.to_bits().hash(&mut h);
        max_h.to_bits().hash(&mut h);
        let key = h.finish();
        if let Some((_, l)) = self.md_cache.borrow().iter().find(|(k, _)| *k == key) {
            return Some(l.clone());
        }
        let layout = Rc::new(self.layout_markdown(md, max_w.max(40.0), max_h, highlight)?);
        let mut cache = self.md_cache.borrow_mut();
        if cache.len() >= 4 {
            cache.remove(0);
        }
        cache.push((key, layout.clone()));
        Some(layout)
    }

    fn layout_markdown(&self, md: &str, max_w: f32, max_h: f32, highlight: Highlight) -> Option<MdLayout> {
        let f = self.fonts.clone()?;
        let lh = f.popup_line_h;
        let gap = (lh * 0.45).round();
        let mut items = Vec::new();
        let mut y = 0.0_f32;
        let mut w = 0.0_f32;
        let mut previous_item = false;
        for (i, block) in parse(md).iter().enumerate() {
            if y >= max_h {
                break;
            }
            let is_item = matches!(block, Block::Item { .. });
            if i > 0 {
                y += if is_item && previous_item { 2.0 } else { gap };
            }
            previous_item = is_item;
            match block {
                Block::Heading(_, text) | Block::Paragraph(text) => {
                    let bold = matches!(block, Block::Heading(..));
                    let (item, tw, th) = self.inline_item(text, 0.0, y, max_w, bold, Ink::WidgetText)?;
                    items.push(item);
                    w = w.max(tw);
                    y += th;
                }
                Block::Quote(text) => {
                    let (item, tw, th) = self.inline_item(text, 12.0, y, max_w - 12.0, false, Ink::WidgetDim)?;
                    items.push(Item::Fill { l: 0.0, t: y, r: 3.0, b: y + th, ink: Ink::WidgetBorder });
                    items.push(item);
                    w = w.max(tw + 12.0);
                    y += th;
                }
                Block::Item { depth, number, text } => {
                    let indent = *depth as f32 * f.char_w_popup * 2.0;
                    let marker = number.map_or("\u{2022}".to_string(), |n| format!("{n}."));
                    let marker_w = f.char_w_popup * if number.is_some() { 3.0 } else { 2.0 };
                    let (dot, _, _) = self.inline_item(&Inline::plain(marker), indent, y, marker_w, false, Ink::WidgetDim)?;
                    let x = indent + marker_w;
                    let (item, tw, th) = self.inline_item(text, x, y, max_w - x, false, Ink::WidgetText)?;
                    items.push(dot);
                    items.push(item);
                    w = w.max(x + tw);
                    y += th;
                }
                Block::Code { lang, text } => {
                    let colors = if matches!(lang.as_str(), "text" | "plain" | "txt") { Vec::new() } else { highlight(text) };
                    let (layout, tw, th) = self.text_layout(text, max_w - 16.0)?;
                    let colors = colors
                        .into_iter()
                        .filter(|(r, _)| r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
                        .map(|(r, ink)| (col_to_utf16(text, r.start) as u32, col_to_utf16(text, r.end) as u32, ink))
                        .collect();
                    items.push(Item::Band { t: y, b: y + th + 8.0, ink: Ink::InputBg });
                    items.push(Item::Text { x: 8.0, y: y + 4.0, layout, ink: Ink::Text, colors, code: Vec::new() });
                    w = w.max(tw + 16.0);
                    y += th + 8.0;
                }
                Block::Rule => {
                    items.push(Item::Rule { y: y.round() + 0.5 });
                    y += 1.0;
                }
                Block::Table { header, rows } => {
                    // Popups rarely hold tables: each row is one line, cells apart.
                    for (index, row) in std::iter::once(header).chain(rows).enumerate() {
                        let mut joined = Inline::default();
                        for (c, cell) in row.iter().enumerate() {
                            if c > 0 {
                                joined.text.push_str("   ");
                            }
                            let start = joined.text.len();
                            joined.text.push_str(&cell.text);
                            joined.spans.extend(cell.spans.iter().map(|(r, s)| (r.start + start..r.end + start, *s)));
                        }
                        if index > 0 {
                            y += 2.0;
                        }
                        let (item, tw, th) = self.inline_item(&joined, 0.0, y, max_w, index == 0, Ink::WidgetText)?;
                        items.push(item);
                        w = w.max(tw);
                        y += th;
                    }
                }
            }
        }
        Some(MdLayout {
            items,
            w: w.min(max_w).ceil(),
            h: y.min(max_h).ceil(),
        })
    }

    /// A wrapped layout of `text` in the popup font, with its width and height.
    fn text_layout(&self, text: &str, max_w: f32) -> Option<(IDWriteTextLayout, f32, f32)> {
        let f = self.fonts.as_ref()?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout = unsafe { self.shared.dw.CreateTextLayout(&wide, &f.popup_text, max_w.max(1.0), 1e5).ok()? };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut m).ok()? };
        Some((layout, m.widthIncludingTrailingWhitespace.min(max_w), m.height.max(f.popup_line_h)))
    }

    /// Styled inline text at (x, y): the item, its width and its height.
    fn inline_item(&self, text: &Inline, x: f32, y: f32, max_w: f32, bold: bool, ink: Ink) -> Option<(Item, f32, f32)> {
        let (layout, _, _) = self.text_layout(&text.text, max_w)?;
        let u = |byte: usize| col_to_utf16(&text.text, byte) as u32;
        let mut colors = Vec::new();
        let mut code_ranges = Vec::new();
        unsafe {
            if bold {
                let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range(0, u(text.text.len())));
            }
            for (r, style) in &text.spans {
                let (a, b) = (u(r.start), u(r.end));
                match style {
                    Style::Bold => {
                        let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range(a, b));
                    }
                    Style::Italic => {
                        let _ = layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range(a, b));
                    }
                    Style::Code => code_ranges.push((a, b)),
                    Style::Link => {
                        let _ = layout.SetUnderline(true, range(a, b));
                        colors.push((a, b, Ink::Info));
                    }
                }
            }
        }
        // Styles change widths, so measure after them.
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut m).ok()? };
        let mut code = Vec::new();
        for (a, b) in code_ranges {
            let mut count = 0u32;
            let _ = unsafe { layout.HitTestTextRange(a, b - a, 0.0, 0.0, None, &mut count) };
            let mut hits = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
            if count > 0 && unsafe { layout.HitTestTextRange(a, b - a, 0.0, 0.0, Some(&mut hits), &mut count) }.is_ok() {
                code.extend(hits.iter().map(|h| [h.left - 2.0, h.top + 1.0, h.left + h.width + 2.0, h.top + h.height - 1.0]));
            }
        }
        let lh = self.fonts.as_ref()?.popup_line_h;
        let width = m.widthIncludingTrailingWhitespace.min(max_w);
        let height = m.height.max(lh);
        Some((Item::Text { x, y, layout, ink, colors, code }, width, height))
    }

    /// Draws `md` with its top left at (x, y), across `w`.
    pub(crate) fn draw_markdown(&self, rt: &ID2D1RenderTarget, md: &MdLayout, x: f32, y: f32, w: f32) {
        for item in &md.items {
            match item {
                Item::Text { x: ix, y: iy, layout, ink, colors, code } => {
                    let (tx, ty) = (x + ix, y + iy);
                    for [l, t, r, b] in code {
                        self.fill(rt, tx + l, ty + t, tx + r, ty + b, Ink::InputBg);
                    }
                    for &(a, b, color) in colors {
                        self.color_range(layout, a, b, color);
                    }
                    self.draw_layout(rt, layout, tx, ty, *ink);
                }
                Item::Band { t, b, ink } => self.fill(rt, x, y + t, x + w, y + b, *ink),
                Item::Fill { l, t, r, b, ink } => self.fill(rt, x + l, y + t, x + r, y + b, *ink),
                Item::Rule { y: ry } => self.line(rt, x, y + ry, x + w, y + ry, 1.0, Ink::WidgetBorder),
            }
        }
    }
}
