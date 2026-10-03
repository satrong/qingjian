//! 皮肤文件：`themes/*.toml` 的 `[skin]` 一节，元数据加深浅两个可选分节，解析后与内置主题合并。

use serde::Deserialize;

use crate::theme::{Theme, ThemePatch};

/// 一份皮肤文件的 `[skin]` 表。
///
/// 结构写错（类型不对、键名拼错、缺 `[skin]`）由 [`ThemeFile::from_toml`] 整体返回错误，
/// 调用方记警告回退内置主题；颜色值写错只回退那一个字段，见 [`ThemePatch`]。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeFile {
    /// 显示名；缺省取文件名，设置页用。
    pub name: Option<String>,

    /// 作者，可选。
    pub author: Option<String>,

    /// 一句话说明，可选。
    pub description: Option<String>,

    /// 浅色分节；缺省整节用内置 [`Theme::light`]。
    pub light: Option<ThemePatch>,

    /// 深色分节；缺省整节用内置 [`Theme::dark`]。
    pub dark: Option<ThemePatch>,
}

impl ThemeFile {
    /// 解析一份皮肤文件的全部文本；`[skin]` 表缺失或结构不对返回带行号的 TOML 错误。
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Root {
            skin: ThemeFile,
        }

        Ok(toml::from_str::<Root>(text)?.skin)
    }

    /// 按当前深浅解析成一份完整主题：以对应内置主题为底，逐字段覆盖皮肤分节里写了的值。
    pub fn resolve(&self, dark: bool) -> Theme {
        let mut theme = if dark { Theme::dark() } else { Theme::light() };
        let patch = if dark {
            self.dark.as_ref()
        } else {
            self.light.as_ref()
        };
        if let Some(patch) = patch {
            patch.apply(&mut theme);
        }
        theme
    }
}

#[cfg(test)]
mod tests {
    use crate::color::Color;
    use crate::theme::FontSpec;

    use super::*;

    #[test]
    fn partial_section_overwrites_only_written_fields() {
        let skin = ThemeFile::from_toml(
            r##"
            [skin.dark]
            background = "#101010"
            corner_radius = 12.0
            max_rows = 7

            [skin.dark.text_font]
            size = 17.0
            line_height = 20.0
            "##,
        )
        .unwrap();
        let theme = skin.resolve(true);
        assert_eq!(theme.colors.background, Color::rgb(0x10, 0x10, 0x10));
        assert_eq!(theme.corner_radius, 12.0);
        assert_eq!(theme.max_rows, 7);
        assert_eq!(theme.text_font, FontSpec::new(17.0, 20.0));
        // 没写的字段保持内置深色主题的值
        assert_eq!(theme.colors.text, Theme::dark().colors.text);
        assert_eq!(theme.padding, Theme::dark().padding);
        assert_eq!(theme.text_gamma, Theme::dark().text_gamma);
    }

    #[test]
    fn missing_section_falls_back_to_builtin() {
        let skin = ThemeFile::from_toml(
            r##"
            [skin]
            name = "夜航"

            [skin.light]
            background = "#FAFAFA"
            "##,
        )
        .unwrap();
        // 写了的浅色分节生效，没写的深色分节整节回退内置
        assert_eq!(
            skin.resolve(false).colors.background,
            Color::rgb(0xFA, 0xFA, 0xFA)
        );
        assert_eq!(skin.resolve(true), Theme::dark());
        // 名字解析到了
        assert_eq!(skin.name.as_deref(), Some("夜航"));
    }

    #[test]
    fn invalid_color_value_falls_back_without_error() {
        let skin = ThemeFile::from_toml(
            r##"
            [skin.dark]
            background = "red"
            gloss = "#RGB"
            highlight = "#244C24FF"
            "##,
        )
        .unwrap();
        let theme = skin.resolve(true);
        // 两个错色值回退内置默认，正确的那个照常生效
        assert_eq!(theme.colors.background, Theme::dark().colors.background);
        assert_eq!(theme.colors.gloss, Theme::dark().colors.gloss);
        assert_eq!(theme.colors.highlight, Color::rgba(0x24, 0x4C, 0x24, 0xFF));
    }

    #[test]
    fn structural_errors_fail_with_line_info() {
        // 缺 [skin] 表
        assert!(ThemeFile::from_toml("name = \"x\"\n").is_err());
        // 分节里键名拼错（deny_unknown_fields）
        assert!(ThemeFile::from_toml("[skin.dark]\nbackgroud = \"#FFF\"\n").is_err());
        // 类型不对
        assert!(ThemeFile::from_toml("[skin.dark]\nmax_rows = \"7\"\n").is_err());
        // 字体子表键名拼错
        assert!(ThemeFile::from_toml("[skin.dark.text_font]\nsze = 17.0\n").is_err());
        // 空文件不 panic
        assert!(ThemeFile::from_toml("").is_err());
        // 深浅分节都缺：两套都回退内置
        let skin = ThemeFile::from_toml("[skin]\nname = \"空\"\n").unwrap();
        assert_eq!(skin.resolve(true), Theme::dark());
        assert_eq!(skin.resolve(false), Theme::light());
    }

    #[test]
    fn light_and_dark_resolve_independently() {
        let skin = ThemeFile::from_toml(
            r##"
            [skin.light]
            background = "#FFFFFF"
            [skin.dark]
            background = "#000000"
            "##,
        )
        .unwrap();
        assert_eq!(
            skin.resolve(false).colors.background,
            Color::rgb(255, 255, 255)
        );
        assert_eq!(skin.resolve(true).colors.background, Color::rgb(0, 0, 0));
    }
}
