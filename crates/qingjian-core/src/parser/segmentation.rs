use std::fmt;

use qingjian_dictionary::{SyllablePattern, canonical_syllable};

/// 切分出的一个音节。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Syllable {
    /// 用户敲的字母。
    pub text: String,

    /// 是完整音节；否则是声母（简拼 `k`）或未打完的前缀（`zho`）。
    pub complete: bool,

    /// 音节后敲的调号（1–5，见 [`crate::parser::is_tone_mark`]）；没敲为 `None`。
    pub tone: Option<u8>,
}

impl Syllable {
    pub fn complete(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            complete: true,
            tone: None,
        }
    }

    pub fn partial(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            complete: false,
            tone: None,
        }
    }

    /// 给这个音节挂上调号。
    pub fn with_tone(mut self, tone: u8) -> Self {
        self.tone = Some(tone);
        self
    }

    pub fn pattern(&self) -> SyllablePattern<'_> {
        SyllablePattern {
            text: canonical_syllable(&self.text),
            complete: self.complete,
        }
    }
}

/// 一种切分方式。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Segmentation {
    /// 音节，顺序与输入一致。
    pub syllables: Vec<Syllable>,
}

impl Segmentation {
    pub fn patterns(&self) -> Vec<SyllablePattern<'_>> {
        self.syllables.iter().map(Syllable::pattern).collect()
    }

    /// 覆盖的输入字母数（不含 `'`）。
    pub fn letters(&self) -> usize {
        self.syllables.iter().map(|s| s.text.len()).sum()
    }

    pub fn incomplete_count(&self) -> usize {
        self.syllables.iter().filter(|s| !s.complete).count()
    }

    pub fn last_is_partial(&self) -> bool {
        self.syllables.last().is_some_and(|s| !s.complete)
    }

    /// 用分隔符连接各音节的字母（不含调号），给查词键、学习键用：`kai'fa`、`k'f`。
    /// 显示用 [`Self::marked`]，那个带调号。
    pub fn joined(&self, separator: &str) -> String {
        self.syllables
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(separator)
    }

    /// 用分隔符连接各音节给 marked text 用：每个音节是它的字母连同跟在后面的调号
    /// （`kai'fa-`、`k'f`、`ni-'hao`）。用户敲的调号要看得见，才知道标的是哪一声。
    pub fn marked(&self, separator: &str) -> String {
        let mut marked = String::new();
        for (index, syllable) in self.syllables.iter().enumerate() {
            if index > 0 {
                marked.push_str(separator);
            }
            marked.push_str(&syllable.text);
            if let Some(tone) = syllable.tone {
                marked.push(crate::parser::tone_char(tone));
            }
        }
        marked
    }
}

impl fmt::Display for Segmentation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, syllable) in self.syllables.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            f.write_str(&syllable.text)?;
            if !syllable.complete {
                f.write_str("…")?;
            }
            if let Some(tone) = syllable.tone {
                write!(f, "{}", crate::parser::tone_char(tone))?;
            }
        }
        Ok(())
    }
}
