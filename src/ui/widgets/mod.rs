//! Reusable controls drawn with the platform canvas: text editors, lists,
//! trees, buttons, and scroll bars.

pub mod editor;
pub mod hosted_editor;
pub mod list;
pub mod scroll;
pub mod sql_highlight;
pub mod tree;

use std::sync::atomic::{AtomicU64, Ordering};

use crate::ui::platform::{Align, Canvas, Color, Cursor, Font, Rect, Weight, rect, text};

/// Identifies a focusable control across frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WidgetId(pub u64);

impl WidgetId {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for WidgetId {
    fn default() -> Self {
        Self::new()
    }
}

/// The colors of the app's chrome; the grid has its own theme.
pub mod colors {
    use crate::ui::platform::Color;

    pub const BACKGROUND: Color = Color::rgb(0xffffff);
    pub const FOREGROUND: Color = Color::rgb(0x1f2328);
    pub const MUTED: Color = Color::rgb(0x6b7280);
    pub const BORDER: Color = Color::rgb(0xdfe1e5);
    pub const INPUT_BORDER: Color = Color::rgb(0xcfd3d9);
    pub const FOCUS: Color = Color::rgb(0x3b82f6);
    pub const HOVER: Color = Color::rgb(0xf1f3f5);
    pub const SELECTED: Color = Color::rgb(0xdbeafe);
    pub const SELECTED_STRONG: Color = Color::rgb(0x2563eb);
    pub const POPOVER: Color = Color::rgb(0xffffff);
    pub const SIDEBAR: Color = Color::rgb(0xfafafa);
    pub const PRIMARY: Color = Color::rgb(0x1f2328);
    pub const PRIMARY_FOREGROUND: Color = Color::rgb(0xffffff);
    pub const DANGER: Color = Color::rgb(0xb33c3c);
    pub const DANGER_BUTTON: Color = Color::rgb(0xdc2626);
    pub const WARNING: Color = Color::rgb(0x9a6b00);
    pub const STRIPE: Color = Color::rgb(0xf5f6f7);
    pub const OVERLAY: Color = Color::rgba(0x00000040);
    pub const KEY_BACKGROUND: Color = Color::rgb(0xf0f1f3);
}

/// The fonts a window draws with, from the user's font settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Fonts {
    pub ui: Font,
    pub small: Font,
    pub tiny: Font,
    pub mono: Font,
    pub large: Font,
}

impl Fonts {
    pub fn new(size: f32, family: Option<&str>, editor_family: Option<&str>) -> Self {
        let family = family.unwrap_or(DEFAULT_FAMILY);
        let ui = Font::new(family, size);
        Self {
            small: ui.sized((size * 0.875).round()),
            tiny: ui.sized((size * 0.75).round()),
            large: ui.sized((size * 1.15).round()).semibold(),
            mono: Font::new(editor_family.unwrap_or(DEFAULT_EDITOR_FAMILY), size),
            ui,
        }
    }

    pub fn line_height(&self) -> f32 {
        text::line_height(&self.ui)
    }
}

pub const DEFAULT_FAMILY: &str = "Segoe UI";
pub const DEFAULT_EDITOR_FAMILY: &str = "Cascadia Mono";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    /// A bordered button.
    Outline,
    /// No border until hovered.
    Ghost,
    Primary,
    Danger,
    /// A toggle shown pressed.
    Selected,
}

/// The width a button with `label` needs.
pub fn button_width(label: &str, font: &Font) -> f32 {
    text::measure(label, font).0 + 20.0
}

pub fn button_height(font: &Font) -> f32 {
    (text::line_height(font) + 10.0).round()
}

/// Draws a button in `r` and makes it answer clicks with `hit`.
#[allow(clippy::too_many_arguments)]
pub fn button<H: Clone + PartialEq>(
    canvas: &mut Canvas<H>,
    r: Rect,
    label: &str,
    font: &Font,
    style: ButtonStyle,
    enabled: bool,
    hit: H,
) {
    let hovered = canvas.hovered(r);
    let (background, foreground, border) = match style {
        ButtonStyle::Outline => (
            if hovered && enabled {
                colors::HOVER
            } else {
                colors::BACKGROUND
            },
            colors::FOREGROUND,
            Some(colors::BORDER),
        ),
        ButtonStyle::Ghost => (
            if hovered && enabled {
                colors::HOVER
            } else {
                Color::TRANSPARENT
            },
            colors::FOREGROUND,
            None,
        ),
        ButtonStyle::Selected => (colors::SELECTED, colors::FOREGROUND, Some(colors::SELECTED)),
        ButtonStyle::Primary => (
            if hovered && enabled {
                Color::rgb(0x3b4148)
            } else {
                colors::PRIMARY
            },
            colors::PRIMARY_FOREGROUND,
            None,
        ),
        ButtonStyle::Danger => (
            if hovered && enabled {
                Color::rgb(0xb91c1c)
            } else {
                colors::DANGER_BUTTON
            },
            colors::PRIMARY_FOREGROUND,
            None,
        ),
    };
    let foreground = if enabled {
        foreground
    } else {
        foreground.opacity(0.4)
    };
    canvas.fill_rounded(r, 5.0, background);
    if let Some(border) = border {
        canvas.stroke_rounded(r, 5.0, border, 1.0);
    }
    canvas.text(r.inset_xy(8.0, 0.0), label, font, foreground, Align::Center);
    if enabled {
        canvas.hit_cursor(r, hit, Cursor::Arrow);
    }
}

/// Draws a checkbox with `label` from `x`, centered in `r`'s height.
/// Returns the width used.
#[allow(clippy::too_many_arguments)]
pub fn checkbox<H: Clone + PartialEq>(
    canvas: &mut Canvas<H>,
    r: Rect,
    label: &str,
    checked: bool,
    font: &Font,
    hit: H,
) -> f32 {
    let size = 15.0;
    let hovered = canvas.hovered(r);
    let boxed = rect(r.x, r.y + (r.h - size) / 2.0, size, size);
    canvas.fill_rounded(
        boxed,
        3.0,
        if checked {
            colors::PRIMARY
        } else if hovered {
            colors::HOVER
        } else {
            colors::BACKGROUND
        },
    );
    canvas.stroke_rounded(
        boxed,
        3.0,
        if checked {
            colors::PRIMARY
        } else {
            colors::INPUT_BORDER
        },
        1.0,
    );
    if checked {
        let x = boxed.x;
        let y = boxed.y;
        canvas.line(
            crate::ui::platform::point(x + 3.5, y + 7.5),
            crate::ui::platform::point(x + 6.5, y + 10.5),
            colors::PRIMARY_FOREGROUND,
            1.8,
        );
        canvas.line(
            crate::ui::platform::point(x + 6.5, y + 10.5),
            crate::ui::platform::point(x + 11.5, y + 4.5),
            colors::PRIMARY_FOREGROUND,
            1.8,
        );
    }
    let label_width = text::measure(label, font).0;
    canvas.text(
        rect(boxed.right() + 6.0, r.y, label_width + 2.0, r.h),
        label,
        font,
        colors::FOREGROUND,
        Align::Left,
    );
    let width = size + 6.0 + label_width + 2.0;
    canvas.hit(rect(r.x, r.y, width, r.h), hit);
    width
}

/// A keystroke drawn as a key cap. Returns its width.
pub fn key_cap<H: Clone>(canvas: &mut Canvas<H>, x: f32, y: f32, h: f32, stroke: &str, font: &Font) -> f32 {
    let label = crate::ui::platform::keys::display(stroke);
    let width = text::measure(&label, font).0 + 10.0;
    let r = rect(x, y, width, h);
    canvas.fill_rounded(r, 4.0, colors::KEY_BACKGROUND);
    canvas.stroke_rounded(r, 4.0, colors::BORDER, 1.0);
    canvas.text(r, &label, font, colors::FOREGROUND, Align::Center);
    width
}

/// How often a [`thinking_dots`] indicator wants a new frame.
pub const THINKING_FRAME: std::time::Duration = std::time::Duration::from_millis(60);

/// A busy indicator: a 3×3 matrix of dots whose lights wander across it.
/// `elapsed` is the time since the work started; repaint every
/// [`THINKING_FRAME`] while it runs.
pub fn thinking_dots<H: Clone>(canvas: &mut Canvas<H>, r: Rect, elapsed: std::time::Duration, color: Color) {
    const SIZE: usize = 3;
    let cell = (r.w.min(r.h) / SIZE as f32).floor().max(3.0);
    let dot = (cell * 0.7).round().max(2.0);
    let left = r.x + (r.w - cell * SIZE as f32) / 2.0;
    let top = r.y + (r.h - cell * SIZE as f32) / 2.0;
    let t = elapsed.as_secs_f32();
    for row in 0..SIZE {
        for column in 0..SIZE {
            let index = (row * SIZE + column) as f32;
            // A wave rolling down the diagonal, with each dot a little out of
            // step so the lit dots seem to move around the matrix.
            let phase = t * 1.6 - (row + column) as f32 * 0.17 + (index * 2.39).sin() * 0.35;
            let wave = 0.5 + 0.5 * (phase * std::f32::consts::TAU).sin();
            let x = left + column as f32 * cell + (cell - dot) / 2.0;
            let y = top + row as f32 * cell + (cell - dot) / 2.0;
            canvas.fill_rounded(rect(x, y, dot, dot), dot / 2.0, color.opacity(0.15 + 0.85 * wave * wave));
        }
    }
}

/// The bold variant of `font`.
pub fn bold(font: &Font) -> Font {
    font.weight(Weight::Bold)
}

/// Lays out rows down a column: each call takes the next slice of height.
pub struct Column {
    pub area: Rect,
    pub y: f32,
    pub gap: f32,
}

impl Column {
    pub fn new(area: Rect, gap: f32) -> Self {
        Self {
            area,
            y: area.y,
            gap,
        }
    }

    /// The next `height` of the column.
    pub fn next(&mut self, height: f32) -> Rect {
        let r = rect(self.area.x, self.y, self.area.w, height);
        self.y += height + self.gap;
        r
    }

    pub fn skip(&mut self, height: f32) {
        self.y += height;
    }

    /// What is left of the column.
    pub fn rest(&self) -> Rect {
        rect(
            self.area.x,
            self.y,
            self.area.w,
            (self.area.bottom() - self.y).max(0.0),
        )
    }

    pub fn used(&self) -> f32 {
        (self.y - self.area.y - self.gap).max(0.0)
    }
}

/// Lays out items across a row from the left or the right.
pub struct Row {
    pub area: Rect,
    left: f32,
    right: f32,
    pub gap: f32,
}

impl Row {
    pub fn new(area: Rect, gap: f32) -> Self {
        Self {
            area,
            left: area.x,
            right: area.right(),
            gap,
        }
    }

    pub fn left(&mut self, width: f32) -> Rect {
        let r = rect(self.left, self.area.y, width.min((self.right - self.left).max(0.0)), self.area.h);
        self.left += width + self.gap;
        r
    }

    pub fn right(&mut self, width: f32) -> Rect {
        let width = width.min((self.right - self.left).max(0.0));
        let r = rect(self.right - width, self.area.y, width, self.area.h);
        self.right -= width + self.gap;
        r
    }

    /// The space between the items placed so far.
    pub fn rest(&self) -> Rect {
        rect(self.left, self.area.y, (self.right - self.left).max(0.0), self.area.h)
    }
}
