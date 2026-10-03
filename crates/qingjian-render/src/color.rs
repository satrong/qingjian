//! 颜色：sRGB 8 位 + alpha，与平台无关；到 tiny-skia / cosmic-text 的换算集中在这里。

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,

    pub g: u8,

    pub b: u8,

    /// 不透明度，255 为完全不透明。
    pub a: u8,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// 纯黑 / 纯白带不透明度，系统语义色（label / secondaryLabel…）都是这个形态。
    pub const fn gray(level: u8, alpha: u8) -> Self {
        Self {
            r: level,
            g: level,
            b: level,
            a: alpha,
        }
    }

    /// 解析 `#RRGGBB` / `#RRGGBBAA`（大小写都认）；`#RGB` 短写不认，写法不对返回 `None`。
    pub fn from_hex(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#')?;
        let bytes = hex.as_bytes();
        if !matches!(bytes.len(), 6 | 8) || !bytes.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
        let a = if bytes.len() == 8 { byte(6)? } else { 255 };
        Some(Self::rgba(byte(0)?, byte(2)?, byte(4)?, a))
    }

    /// `#RRGGBB` / `#RRGGBBAA`；给皮肤文件与设置页显示用，与 [`Color::from_hex`] 互逆。
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
        }
    }

    pub(crate) fn to_skia(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba8(self.r, self.g, self.b, self.a)
    }

    pub(crate) fn to_cosmic(self) -> cosmic_text::Color {
        cosmic_text::Color::rgba(self.r, self.g, self.b, self.a)
    }

    /// 乘上一层覆盖率（字形遮罩的像素值）后的预乘颜色。
    pub(crate) fn premultiplied(self, coverage: u8) -> tiny_skia::PremultipliedColorU8 {
        let alpha = mul_u8(self.a, coverage);
        premultiply(self.r, self.g, self.b, alpha)
    }
}

/// 8 位定点乘法：`a * b / 255`，四舍五入。
pub(crate) fn mul_u8(a: u8, b: u8) -> u8 {
    ((u32::from(a) * u32::from(b) + 127) / 255) as u8
}

/// 直通 RGBA → 预乘。
pub(crate) fn premultiply(r: u8, g: u8, b: u8, a: u8) -> tiny_skia::PremultipliedColorU8 {
    tiny_skia::PremultipliedColorU8::from_rgba(mul_u8(r, a), mul_u8(g, a), mul_u8(b, a), a)
        .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT)
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::from_hex(&text)
            .ok_or_else(|| D::Error::custom(format_args!("颜色不是 #RRGGBB 或 #RRGGBBAA：{text}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplies_with_coverage() {
        let c = Color::rgba(255, 0, 0, 255).premultiplied(128);
        assert_eq!((c.red(), c.green(), c.blue(), c.alpha()), (128, 0, 0, 128));
        let half = Color::gray(0, 128).premultiplied(255);
        assert_eq!((half.red(), half.alpha()), (0, 128));
        assert_eq!(mul_u8(255, 255), 255);
        assert_eq!(mul_u8(0, 255), 0);
    }

    #[test]
    fn parses_hex_colors() {
        assert_eq!(Color::from_hex("#FFFFFF"), Some(Color::rgb(255, 255, 255)));
        assert_eq!(Color::from_hex("#ffffff"), Some(Color::rgb(255, 255, 255)));
        assert_eq!(
            Color::from_hex("#1E1E1EFF"),
            Some(Color::rgba(30, 30, 30, 255))
        );
        assert_eq!(
            Color::from_hex("#FF00807F"),
            Some(Color::rgba(255, 0, 128, 127))
        );
        // #RGB 短写、缺 #、位数不对、非十六进制都拒绝
        assert_eq!(Color::from_hex("#FFF"), None);
        assert_eq!(Color::from_hex("FFFFFF"), None);
        assert_eq!(Color::from_hex("#FFFFF"), None);
        assert_eq!(Color::from_hex("#FFFFFG"), None);
        assert_eq!(Color::from_hex(""), None);
        assert_eq!(Color::from_hex("#1234567890"), None);
    }

    #[test]
    fn hex_round_trips() {
        for color in [
            Color::rgb(255, 255, 255),
            Color::rgba(30, 30, 30, 255),
            Color::rgba(255, 0, 128, 127),
        ] {
            assert_eq!(Color::from_hex(&color.to_hex()), Some(color));
        }
        assert_eq!(Color::rgb(30, 30, 30).to_hex(), "#1E1E1E");
        assert_eq!(Color::rgba(30, 30, 30, 128).to_hex(), "#1E1E1E80");
    }

    #[test]
    fn serde_uses_hex_string() {
        #[derive(Deserialize, Serialize, PartialEq, Debug)]
        struct Wrapper {
            color: Color,
        }

        let text = "color = \"#FF00807F\"\n";
        let wrapper: Wrapper = toml::from_str(text).unwrap();
        assert_eq!(wrapper.color, Color::rgba(255, 0, 128, 127));
        assert_eq!(toml::to_string(&wrapper).unwrap(), text);

        assert!(toml::from_str::<Wrapper>("color = \"#FFF\"\n").is_err());
    }
}
