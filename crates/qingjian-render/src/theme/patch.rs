//! 皮肤文件里的一个分节（`[skin.light]` / `[skin.dark]`）：写了的字段覆盖，没写的回退内置默认。

use serde::{Deserialize, Deserializer};

use crate::color::Color;
use crate::theme::{FontSpec, Theme};

/// 皮肤的一个分节。颜色字段平铺在分节下（`background = "#1E1E1EFF"`），
/// 与设计工具的输出对齐；字体是 `text_font` 等三个子表，`size` / `line_height` 一起给。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemePatch {
    /// 候选词。
    #[serde(deserialize_with = "lenient_color")]
    pub text: Option<Color>,

    /// 译文。
    #[serde(deserialize_with = "lenient_color")]
    pub gloss: Option<Color>,

    /// 词性，比译文更浅。
    #[serde(deserialize_with = "lenient_color")]
    pub pos: Option<Color>,

    /// 生词译文。
    #[serde(deserialize_with = "lenient_color")]
    pub fresh: Option<Color>,

    /// 序号。
    #[serde(deserialize_with = "lenient_color")]
    pub index: Option<Color>,

    /// 云联想的云朵与文字。
    #[serde(deserialize_with = "lenient_color")]
    pub cloud: Option<Color>,

    /// 窗口背景。
    #[serde(deserialize_with = "lenient_color")]
    pub background: Option<Color>,

    /// 当前候选的高亮底色。
    #[serde(deserialize_with = "lenient_color")]
    pub highlight: Option<Color>,

    /// 候选词字体。
    pub text_font: Option<FontSpec>,

    /// 译文与词性字体。
    pub annotation_font: Option<FontSpec>,

    /// 序号字体。
    pub index_font: Option<FontSpec>,

    /// 窗口内边距。
    pub padding: Option<f32>,

    /// 行内上下留白。
    pub row_padding: Option<f32>,

    /// 序号与候选词、候选词与译文之间的间距。
    pub column_gap: Option<f32>,

    /// 窗口与高亮条的圆角。
    pub corner_radius: Option<f32>,

    /// 最多显示几行。
    pub max_rows: Option<usize>,

    /// 文字抗锯齿覆盖率的 gamma，见 [`Theme::text_gamma`]；皮肤里开放但不推荐改。
    pub text_gamma: Option<f32>,
}

impl ThemePatch {
    /// 把这一节写了的字段盖到 `theme` 上；没写的保持 `theme` 原值。
    pub fn apply(&self, theme: &mut Theme) {
        if let Some(color) = self.text {
            theme.colors.text = color;
        }
        if let Some(color) = self.gloss {
            theme.colors.gloss = color;
        }
        if let Some(color) = self.pos {
            theme.colors.pos = color;
        }
        if let Some(color) = self.fresh {
            theme.colors.fresh = color;
        }
        if let Some(color) = self.index {
            theme.colors.index = color;
        }
        if let Some(color) = self.cloud {
            theme.colors.cloud = color;
        }
        if let Some(color) = self.background {
            theme.colors.background = color;
        }
        if let Some(color) = self.highlight {
            theme.colors.highlight = color;
        }
        if let Some(font) = self.text_font {
            theme.text_font = font;
        }
        if let Some(font) = self.annotation_font {
            theme.annotation_font = font;
        }
        if let Some(font) = self.index_font {
            theme.index_font = font;
        }
        if let Some(value) = self.padding {
            theme.padding = value;
        }
        if let Some(value) = self.row_padding {
            theme.row_padding = value;
        }
        if let Some(value) = self.column_gap {
            theme.column_gap = value;
        }
        if let Some(value) = self.corner_radius {
            theme.corner_radius = value;
        }
        if let Some(value) = self.max_rows {
            theme.max_rows = value;
        }
        if let Some(value) = self.text_gamma {
            theme.text_gamma = value;
        }
    }
}

/// 颜色值写错只回退这一个字段（记警告），不让整份皮肤失效——打不开候选窗才是大事。
fn lenient_color<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Color>, D::Error> {
    let text = String::deserialize(deserializer)?;
    match Color::from_hex(&text) {
        Some(color) => Ok(Some(color)),
        None => {
            tracing::warn!("皮肤颜色值不是 #RRGGBB(AA)，回退默认：{text}");
            Ok(None)
        }
    }
}
