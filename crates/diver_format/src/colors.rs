//! Colors: the CSS named colors, hex literals and the blends color scales use.

/// An sRGB color with alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color { r, g, b, a: 255 }
    }

    const fn hex(rgb: u32) -> Self {
        Color::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }

    /// `0xAARRGGBB`, the editor's decoration format.
    pub fn argb(self) -> u32 {
        (self.a as u32) << 24 | (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }

    /// Components from 0 to 1, for Direct2D.
    pub fn floats(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a].map(|c| c as f32 / 255.0)
    }

    /// `#rrggbb`, or `#rrggbbaa` when not opaque.
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// Parses `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` (the `#` is optional).
    pub fn parse_hex(text: &str) -> Option<Color> {
        let digits = text.strip_prefix('#').unwrap_or(text);
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let nibble = |i: usize| u8::from_str_radix(&digits[i..i + 1], 16).ok().map(|n| n * 17);
        let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
        Some(match digits.len() {
            3 => Color::rgb(nibble(0)?, nibble(1)?, nibble(2)?),
            4 => Color { a: nibble(3)?, ..Color::rgb(nibble(0)?, nibble(1)?, nibble(2)?) },
            6 => Color::rgb(byte(0)?, byte(2)?, byte(4)?),
            8 => Color { a: byte(6)?, ..Color::rgb(byte(0)?, byte(2)?, byte(4)?) },
            _ => return None,
        })
    }

    /// A named color, ignoring case.
    pub fn named(name: &str) -> Option<Color> {
        NAMED
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, c)| *c)
    }

    /// The color `t` of the way from `self` to `other`.
    pub fn mix(self, other: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let blend = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color {
            r: blend(self.r, other.r),
            g: blend(self.g, other.g),
            b: blend(self.b, other.b),
            a: blend(self.a, other.a),
        }
    }

    /// Relative luminance, 0 (black) to 1 (white).
    pub fn luminance(self) -> f32 {
        let channel = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// Black or white, whichever reads better on this color.
    pub fn contrast_text(self) -> Color {
        if self.luminance() > 0.3 { Color::rgb(0, 0, 0) } else { Color::rgb(255, 255, 255) }
    }
}

/// The color at `t` (0 to 1) along evenly spaced stops.
pub fn along(stops: &[Color], t: f32) -> Color {
    match stops {
        [] => Color::rgb(0, 0, 0),
        [only] => *only,
        _ => {
            let t = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
            let i = (t.floor() as usize).min(stops.len() - 2);
            stops[i].mix(stops[i + 1], t - i as f32)
        }
    }
}

/// The colors the `#` picker offers before anything is typed: Excel's
/// conditional-format fills and text colors, then a spread of hues.
pub const PALETTE: &[(&str, Color)] = &[
    ("light red fill", Color::hex(0xffc7ce)),
    ("dark red text", Color::hex(0x9c0006)),
    ("light yellow fill", Color::hex(0xffeb9c)),
    ("dark yellow text", Color::hex(0x9c5700)),
    ("light green fill", Color::hex(0xc6efce)),
    ("dark green text", Color::hex(0x006100)),
    ("scale red", Color::hex(0xf8696b)),
    ("scale yellow", Color::hex(0xffeb84)),
    ("scale green", Color::hex(0x63be7b)),
    ("bar blue", Color::hex(0x638ec6)),
    ("bar orange", Color::hex(0xffb628)),
    ("bar purple", Color::hex(0xd6007b)),
    ("light gray fill", Color::hex(0xeeeeee)),
    ("gray text", Color::hex(0x808080)),
    ("red", Color::hex(0xe53935)),
    ("orange", Color::hex(0xfb8c00)),
    ("amber", Color::hex(0xffb300)),
    ("green", Color::hex(0x43a047)),
    ("teal", Color::hex(0x00897b)),
    ("blue", Color::hex(0x1e88e5)),
    ("indigo", Color::hex(0x3949ab)),
    ("purple", Color::hex(0x8e24aa)),
    ("pink", Color::hex(0xd81b60)),
    ("brown", Color::hex(0x6d4c41)),
];

/// The CSS named colors.
pub const NAMED: &[(&str, Color)] = &[
    ("aliceblue", Color::hex(0xf0f8ff)),
    ("antiquewhite", Color::hex(0xfaebd7)),
    ("aqua", Color::hex(0x00ffff)),
    ("aquamarine", Color::hex(0x7fffd4)),
    ("azure", Color::hex(0xf0ffff)),
    ("beige", Color::hex(0xf5f5dc)),
    ("bisque", Color::hex(0xffe4c4)),
    ("black", Color::hex(0x000000)),
    ("blanchedalmond", Color::hex(0xffebcd)),
    ("blue", Color::hex(0x0000ff)),
    ("blueviolet", Color::hex(0x8a2be2)),
    ("brown", Color::hex(0xa52a2a)),
    ("burlywood", Color::hex(0xdeb887)),
    ("cadetblue", Color::hex(0x5f9ea0)),
    ("chartreuse", Color::hex(0x7fff00)),
    ("chocolate", Color::hex(0xd2691e)),
    ("coral", Color::hex(0xff7f50)),
    ("cornflowerblue", Color::hex(0x6495ed)),
    ("cornsilk", Color::hex(0xfff8dc)),
    ("crimson", Color::hex(0xdc143c)),
    ("cyan", Color::hex(0x00ffff)),
    ("darkblue", Color::hex(0x00008b)),
    ("darkcyan", Color::hex(0x008b8b)),
    ("darkgoldenrod", Color::hex(0xb8860b)),
    ("darkgray", Color::hex(0xa9a9a9)),
    ("darkgreen", Color::hex(0x006400)),
    ("darkgrey", Color::hex(0xa9a9a9)),
    ("darkkhaki", Color::hex(0xbdb76b)),
    ("darkmagenta", Color::hex(0x8b008b)),
    ("darkolivegreen", Color::hex(0x556b2f)),
    ("darkorange", Color::hex(0xff8c00)),
    ("darkorchid", Color::hex(0x9932cc)),
    ("darkred", Color::hex(0x8b0000)),
    ("darksalmon", Color::hex(0xe9967a)),
    ("darkseagreen", Color::hex(0x8fbc8f)),
    ("darkslateblue", Color::hex(0x483d8b)),
    ("darkslategray", Color::hex(0x2f4f4f)),
    ("darkslategrey", Color::hex(0x2f4f4f)),
    ("darkturquoise", Color::hex(0x00ced1)),
    ("darkviolet", Color::hex(0x9400d3)),
    ("deeppink", Color::hex(0xff1493)),
    ("deepskyblue", Color::hex(0x00bfff)),
    ("dimgray", Color::hex(0x696969)),
    ("dimgrey", Color::hex(0x696969)),
    ("dodgerblue", Color::hex(0x1e90ff)),
    ("firebrick", Color::hex(0xb22222)),
    ("floralwhite", Color::hex(0xfffaf0)),
    ("forestgreen", Color::hex(0x228b22)),
    ("fuchsia", Color::hex(0xff00ff)),
    ("gainsboro", Color::hex(0xdcdcdc)),
    ("ghostwhite", Color::hex(0xf8f8ff)),
    ("gold", Color::hex(0xffd700)),
    ("goldenrod", Color::hex(0xdaa520)),
    ("gray", Color::hex(0x808080)),
    ("green", Color::hex(0x008000)),
    ("greenyellow", Color::hex(0xadff2f)),
    ("grey", Color::hex(0x808080)),
    ("honeydew", Color::hex(0xf0fff0)),
    ("hotpink", Color::hex(0xff69b4)),
    ("indianred", Color::hex(0xcd5c5c)),
    ("indigo", Color::hex(0x4b0082)),
    ("ivory", Color::hex(0xfffff0)),
    ("khaki", Color::hex(0xf0e68c)),
    ("lavender", Color::hex(0xe6e6fa)),
    ("lavenderblush", Color::hex(0xfff0f5)),
    ("lawngreen", Color::hex(0x7cfc00)),
    ("lemonchiffon", Color::hex(0xfffacd)),
    ("lightblue", Color::hex(0xadd8e6)),
    ("lightcoral", Color::hex(0xf08080)),
    ("lightcyan", Color::hex(0xe0ffff)),
    ("lightgoldenrodyellow", Color::hex(0xfafad2)),
    ("lightgray", Color::hex(0xd3d3d3)),
    ("lightgreen", Color::hex(0x90ee90)),
    ("lightgrey", Color::hex(0xd3d3d3)),
    ("lightpink", Color::hex(0xffb6c1)),
    ("lightsalmon", Color::hex(0xffa07a)),
    ("lightseagreen", Color::hex(0x20b2aa)),
    ("lightskyblue", Color::hex(0x87cefa)),
    ("lightslategray", Color::hex(0x778899)),
    ("lightslategrey", Color::hex(0x778899)),
    ("lightsteelblue", Color::hex(0xb0c4de)),
    ("lightyellow", Color::hex(0xffffe0)),
    ("lime", Color::hex(0x00ff00)),
    ("limegreen", Color::hex(0x32cd32)),
    ("linen", Color::hex(0xfaf0e6)),
    ("magenta", Color::hex(0xff00ff)),
    ("maroon", Color::hex(0x800000)),
    ("mediumaquamarine", Color::hex(0x66cdaa)),
    ("mediumblue", Color::hex(0x0000cd)),
    ("mediumorchid", Color::hex(0xba55d3)),
    ("mediumpurple", Color::hex(0x9370db)),
    ("mediumseagreen", Color::hex(0x3cb371)),
    ("mediumslateblue", Color::hex(0x7b68ee)),
    ("mediumspringgreen", Color::hex(0x00fa9a)),
    ("mediumturquoise", Color::hex(0x48d1cc)),
    ("mediumvioletred", Color::hex(0xc71585)),
    ("midnightblue", Color::hex(0x191970)),
    ("mintcream", Color::hex(0xf5fffa)),
    ("mistyrose", Color::hex(0xffe4e1)),
    ("moccasin", Color::hex(0xffe4b5)),
    ("navajowhite", Color::hex(0xffdead)),
    ("navy", Color::hex(0x000080)),
    ("oldlace", Color::hex(0xfdf5e6)),
    ("olive", Color::hex(0x808000)),
    ("olivedrab", Color::hex(0x6b8e23)),
    ("orange", Color::hex(0xffa500)),
    ("orangered", Color::hex(0xff4500)),
    ("orchid", Color::hex(0xda70d6)),
    ("palegoldenrod", Color::hex(0xeee8aa)),
    ("palegreen", Color::hex(0x98fb98)),
    ("paleturquoise", Color::hex(0xafeeee)),
    ("palevioletred", Color::hex(0xdb7093)),
    ("papayawhip", Color::hex(0xffefd5)),
    ("peachpuff", Color::hex(0xffdab9)),
    ("peru", Color::hex(0xcd853f)),
    ("pink", Color::hex(0xffc0cb)),
    ("plum", Color::hex(0xdda0dd)),
    ("powderblue", Color::hex(0xb0e0e6)),
    ("purple", Color::hex(0x800080)),
    ("rebeccapurple", Color::hex(0x663399)),
    ("red", Color::hex(0xff0000)),
    ("rosybrown", Color::hex(0xbc8f8f)),
    ("royalblue", Color::hex(0x4169e1)),
    ("saddlebrown", Color::hex(0x8b4513)),
    ("salmon", Color::hex(0xfa8072)),
    ("sandybrown", Color::hex(0xf4a460)),
    ("seagreen", Color::hex(0x2e8b57)),
    ("seashell", Color::hex(0xfff5ee)),
    ("sienna", Color::hex(0xa0522d)),
    ("silver", Color::hex(0xc0c0c0)),
    ("skyblue", Color::hex(0x87ceeb)),
    ("slateblue", Color::hex(0x6a5acd)),
    ("slategray", Color::hex(0x708090)),
    ("slategrey", Color::hex(0x708090)),
    ("snow", Color::hex(0xfffafa)),
    ("springgreen", Color::hex(0x00ff7f)),
    ("steelblue", Color::hex(0x4682b4)),
    ("tan", Color::hex(0xd2b48c)),
    ("teal", Color::hex(0x008080)),
    ("thistle", Color::hex(0xd8bfd8)),
    ("tomato", Color::hex(0xff6347)),
    ("transparent", Color { r: 0, g: 0, b: 0, a: 0 }),
    ("turquoise", Color::hex(0x40e0d0)),
    ("violet", Color::hex(0xee82ee)),
    ("wheat", Color::hex(0xf5deb3)),
    ("white", Color::hex(0xffffff)),
    ("whitesmoke", Color::hex(0xf5f5f5)),
    ("yellow", Color::hex(0xffff00)),
    ("yellowgreen", Color::hex(0x9acd32)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_forms_parse() {
        assert_eq!(Color::parse_hex("#f00"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(Color::parse_hex("#00ff0080").map(|c| c.a), Some(0x80));
        assert_eq!(Color::parse_hex("#12345"), None);
        assert_eq!(Color::parse_hex("#ggg"), None);
        assert_eq!(Color::rgb(1, 2, 255).to_hex(), "#0102ff");
    }

    #[test]
    fn names_ignore_case_and_stops_blend() {
        assert_eq!(Color::named("MistyRose"), Some(Color::hex(0xffe4e1)));
        let stops = [Color::rgb(0, 0, 0), Color::rgb(200, 0, 0), Color::rgb(200, 200, 0)];
        assert_eq!(along(&stops, 0.25), Color::rgb(100, 0, 0));
        assert_eq!(along(&stops, 1.0), Color::rgb(200, 200, 0));
    }
}
