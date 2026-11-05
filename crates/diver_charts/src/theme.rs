//! Colors, fonts and mark sizes shared by every chart.

use crate::geom::Color;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    pub size: f32,
    pub bold: bool,
}

impl Font {
    pub const fn new(size: f32, bold: bool) -> Font {
        Font { size, bold }
    }

    /// The height one line takes, for spacing.
    pub fn line(&self) -> f32 {
        (self.size * 1.33).ceil()
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub dark: bool,
    pub surface: Color,
    /// Values and titles.
    pub text: Color,
    /// Legend names and other supporting text.
    pub text2: Color,
    /// Tick labels and axis titles.
    pub muted: Color,
    pub grid: Color,
    pub axis: Color,
    /// The hairline ring of a raised box such as a tooltip.
    pub border: Color,
    pub tooltip: Color,
    /// Series colors, in the order series take them. A ninth series is never
    /// a made-up hue: charts fold extras into "Other".
    pub palette: [Color; 8],
    pub other: Color,
    pub title: Font,
    pub subtitle: Font,
    pub tick: Font,
    pub axis_title: Font,
    pub legend: Font,
    pub label: Font,
    pub tip: Font,
    pub tip_value: Font,
    /// The thickest a bar is drawn; a wider slot leaves the rest as air.
    pub bar_max: f32,
    /// The rounding of a bar's data end.
    pub radius: f32,
    /// The surface-colored gap between touching marks.
    pub gap: f32,
    pub pad: f32,
}

const FONTS: (Font, Font, Font, Font, Font, Font, Font, Font) = (
    Font::new(15.0, true),
    Font::new(12.0, false),
    Font::new(11.0, false),
    Font::new(11.5, false),
    Font::new(12.0, false),
    Font::new(11.0, false),
    Font::new(12.0, false),
    Font::new(12.0, true),
);

impl Theme {
    pub fn light() -> Theme {
        let (title, subtitle, tick, axis_title, legend, label, tip, tip_value) = FONTS;
        Theme {
            dark: false,
            surface: Color::hex(0xfcfcfb),
            text: Color::hex(0x0b0b0b),
            text2: Color::hex(0x52514e),
            muted: Color::hex(0x898781),
            grid: Color::hex(0xe1e0d9),
            axis: Color::hex(0xc3c2b7),
            border: Color::hex(0x0b0b0b).with_alpha(26),
            tooltip: Color::hex(0xffffff),
            palette: [
                Color::hex(0x2a78d6),
                Color::hex(0xeb6834),
                Color::hex(0x1baf7a),
                Color::hex(0xeda100),
                Color::hex(0xe87ba4),
                Color::hex(0x008300),
                Color::hex(0x6250d6),
                Color::hex(0xe34948),
            ],
            other: Color::hex(0xa8a79f),
            title,
            subtitle,
            tick,
            axis_title,
            legend,
            label,
            tip,
            tip_value,
            bar_max: 24.0,
            radius: 4.0,
            gap: 2.0,
            pad: 16.0,
        }
    }

    pub fn dark() -> Theme {
        Theme {
            dark: true,
            surface: Color::hex(0x1a1a19),
            text: Color::hex(0xf0efec),
            text2: Color::hex(0xc3c2b7),
            muted: Color::hex(0x898781),
            grid: Color::hex(0x2c2c2a),
            axis: Color::hex(0x383835),
            border: Color::hex(0xffffff).with_alpha(26),
            tooltip: Color::hex(0x262624),
            palette: [
                Color::hex(0x3987e5),
                Color::hex(0xd95926),
                Color::hex(0x199e70),
                Color::hex(0xc98500),
                Color::hex(0xd55181),
                Color::hex(0x008300),
                Color::hex(0x9085e9),
                Color::hex(0xe66767),
            ],
            other: Color::hex(0x6b6a65),
            ..Theme::light()
        }
    }

    /// The color of series `i`; past the palette, the "Other" gray.
    pub fn series(&self, i: usize) -> Color {
        self.palette.get(i).copied().unwrap_or(self.other)
    }

    /// Text that reads on a fill of `color`.
    pub fn ink_on(&self, color: Color) -> Color {
        if color.luminance() > 0.36 { Color::hex(0x0b0b0b) } else { Color::hex(0xffffff) }
    }

    /// A hovered mark, lifted a little.
    pub fn lifted(&self, color: Color) -> Color {
        color.mix(Color::hex(0xffffff), if self.dark { 0.18 } else { 0.25 })
    }
}
