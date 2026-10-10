use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const THEME_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeColor(u32);

impl ThemeColor {
    pub const fn rgb(value: u32) -> Self {
        Self((value << 8) | 0xff)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let digits = text.strip_prefix('#').unwrap_or(text);
        if !matches!(digits.len(), 6 | 8) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Use #RRGGBB or #RRGGBBAA".into());
        }
        let value = u32::from_str_radix(digits, 16).map_err(|error| error.to_string())?;
        Ok(if digits.len() == 6 {
            Self::rgb(value)
        } else {
            Self(value)
        })
    }

    pub fn rgba(self) -> u32 {
        self.0
    }

    pub fn hex(self) -> String {
        if self.0 & 0xff == 0xff {
            format!("#{:06X}", self.0 >> 8)
        } else {
            format!("#{:08X}", self.0)
        }
    }
}

impl Serialize for ThemeColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for ThemeColor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSettings {
    pub schema_version: u32,
    pub header_background: ThemeColor,
    pub frozen_header_background: ThemeColor,
    pub background: ThemeColor,
    pub row_stripe_background: ThemeColor,
    pub frozen_background: ThemeColor,
    pub frozen_stripe_background: ThemeColor,
    pub foreground: ThemeColor,
    pub grid_border: ThemeColor,
    pub selection_background: ThemeColor,
    pub active_cell_border: ThemeColor,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            schema_version: THEME_SCHEMA_VERSION,
            header_background: ThemeColor::rgb(0xd5dae0),
            frozen_header_background: ThemeColor::rgb(0xbecddd),
            background: ThemeColor::rgb(0xffffff),
            row_stripe_background: ThemeColor::rgb(0xf3f4f6),
            frozen_background: ThemeColor::rgb(0xdfe7ef),
            frozen_stripe_background: ThemeColor::rgb(0xd2deea),
            foreground: ThemeColor::rgb(0x1f2937),
            grid_border: ThemeColor::rgb(0xcbd2da),
            selection_background: ThemeColor::rgb(0xdbeafe),
            active_cell_border: ThemeColor::rgb(0xc9a24a),
        }
    }
}

macro_rules! theme_roles {
    ($( $variant:ident => $field:ident, $label:literal; )*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum ThemeRole { $( $variant, )* }

        impl ThemeRole {
            pub const ALL: [Self; 10] = [$( Self::$variant, )*];

            pub fn label(self) -> &'static str {
                match self { $( Self::$variant => $label, )* }
            }

            pub fn key(self) -> &'static str {
                match self { $( Self::$variant => stringify!($field), )* }
            }

            pub fn color(self, theme: &ThemeSettings) -> ThemeColor {
                match self { $( Self::$variant => theme.$field, )* }
            }

            pub fn set(self, theme: &mut ThemeSettings, color: ThemeColor) {
                match self { $( Self::$variant => theme.$field = color, )* }
            }
        }
    };
}

theme_roles! {
    Header => header_background, "Header background";
    FrozenHeader => frozen_header_background, "Frozen header background";
    Background => background, "Table background";
    RowStripe => row_stripe_background, "Alternating row background";
    Frozen => frozen_background, "Frozen column background";
    FrozenStripe => frozen_stripe_background, "Frozen alternating row background";
    Foreground => foreground, "Table text";
    GridBorder => grid_border, "Grid borders";
    Selection => selection_background, "Selection background";
    ActiveCell => active_cell_border, "Active cell border";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colors_preserve_bytes_and_reject_invalid_input() {
        for text in ["#123456", "#12345680", "#00000000", " aAbBcC "] {
            let color = ThemeColor::parse(text).unwrap();
            let json = serde_json::to_string(&color).unwrap();
            assert_eq!(serde_json::from_str::<ThemeColor>(&json).unwrap(), color);
        }
        assert_eq!(ThemeColor::parse("#12345680").unwrap().rgba(), 0x12345680);
        for text in ["", "#bad", "#GG0000", "#123456789", "é12345"] {
            assert!(ThemeColor::parse(text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn missing_roles_use_defaults_and_all_roles_can_be_changed() {
        let theme: ThemeSettings =
            serde_json::from_str(r##"{"header_background":"#123456"}"##).unwrap();
        assert_eq!(theme.header_background, ThemeColor::rgb(0x123456));
        assert_eq!(
            theme.frozen_background,
            ThemeSettings::default().frozen_background
        );
        for role in ThemeRole::ALL {
            let mut theme = ThemeSettings::default();
            role.set(&mut theme, ThemeColor::rgb(0x123456));
            assert_eq!(role.color(&theme), ThemeColor::rgb(0x123456));
        }
    }
}
