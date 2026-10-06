//! 拼音切分：把无分隔的拼音串切成音节序列。
//!
//! 支持全拼、简拼（声母缩写 `kf` → `k f`）与两者混用（`kaif`、`kfa`），输入允许用 `'` 强制分隔（`xi'an`）。
//! 末尾允许一个未打完的音节前缀（`zho` → `zho…`）。每种切分里每个音节标记是否完整，
//! 查词时完整音节精确匹配、不完整音节按前缀匹配。双拼、模糊音后续在这里扩展，接口保持不变。

mod error;
mod segmentation;
mod syllable;
mod trie;

use std::cmp::Reverse;

pub use error::ParseError;
pub use segmentation::{Segmentation, Syllable};
pub use syllable::{INITIALS, MAX_SYLLABLE_LEN, SYLLABLES};

use syllable::initial_lengths;
use trie::SYLLABLE_TRIE;

/// 声调键：`-` 一声、`/` 二声、`=` 三声、`\` 四声、`.` 轻声。不是调号时为 `None`。
///
/// 调号紧跟在它标注的音节之后，同时充当段界（`ni-hao` ≡ `ni`+`hao`，`ni` 一声）；`'` 的音节分隔语义不变。
pub const fn is_tone_mark(c: char) -> Option<u8> {
    match c {
        '-' => Some(1),
        '/' => Some(2),
        '=' => Some(3),
        '\\' => Some(4),
        '.' => Some(5),
        _ => None,
    }
}

/// 调号（1–5）对应的键，[`is_tone_mark`] 的逆；不认识的值返回 `.`（不 panic，显示路径不该炸）。
pub const fn tone_char(tone: u8) -> char {
    match tone {
        1 => '-',
        2 => '/',
        3 => '=',
        4 => '\\',
        _ => '.',
    }
}

pub fn is_syllable(s: &str) -> bool {
    SYLLABLE_TRIE.contains(s)
}

/// `s` 是否为某个合法音节的**真**前缀（不包含它自己就是完整音节的情况）。
pub fn is_syllable_prefix(s: &str) -> bool {
    SYLLABLE_TRIE.is_proper_prefix(s)
}

/// 含调号的输入读不读得出拼音：每个调号前面得有音节可挂（开头、`'` 后、连续调号都不行），
/// 且去掉调号后切得动、残缺音节只允许在末尾（`ni-h` 读得出，`no-way` 读不出）。
/// 英文直输段的判定用它兜底：调号当了拼音键之后，带调号的乱串仍是直输段。
pub fn segmentable_with_marks(input: &str) -> bool {
    let mut letters = String::with_capacity(input.len());
    let mut since_boundary = 0usize;
    for c in input.chars() {
        if is_tone_mark(c).is_some() {
            if since_boundary == 0 {
                return false;
            }
            since_boundary = 0;
            continue;
        }
        if c == '\'' {
            since_boundary = 0;
        } else if c.is_ascii_lowercase() {
            since_boundary += 1;
        }
        letters.push(c);
    }
    match segment(&letters) {
        Ok(segmentations) => segmentations.iter().any(|segmentation| {
            let incomplete = segmentation.incomplete_count();
            incomplete <= usize::from(segmentation.last_is_partial())
        }),
        Err(_) => false,
    }
}

/// `text` 能否切成每个音节都完整的拼音。只回答是否，不产生切分、不分配音节：
/// 纠错要对上千个变体逐个问这个问题，先用它过滤，剩下的几个再做真正的切分。只认小写字母。
pub fn is_fully_segmentable(text: &str) -> bool {
    let n = text.len();
    if n == 0 {
        return false;
    }
    let mut reachable = vec![false; n + 1];
    reachable[0] = true;
    for start in 0..n {
        if !reachable[start] {
            continue;
        }
        for len in SYLLABLE_TRIE.matches(&text[start..]).lengths() {
            reachable[start + len] = true;
        }
    }
    reachable[n]
}

/// 切分上限。简拼让歧义切分数量指数增长，每个位置只保留这么多种最优切分。
const MAX_SEGMENTATIONS: usize = 8;

/// 切分。返回按「音节少、不完整音节少、前面的音节长」排序的切分，最多 [`MAX_SEGMENTATIONS`] 种。
///
/// 除字母与 `'` 外还接受声调键（[`is_tone_mark`]）：调号紧跟在它标注的音节后、同时充当段界，
/// 挂在该段最后一个音节上（`nihao-` 标的是 `hao`）。调号前后都没有可标的音节时报 `InvalidChar`
/// （`-ni`、`ni--`）。调号只进 [`Syllable::tone`]，不进 `text`，切分键与查词键不受影响。
pub fn segment(input: &str) -> Result<Vec<Segmentation>, ParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ParseError::Empty);
    }
    // 按 `'` 与调号切成若干段；连续 `'` 的空段照旧忽略，调号标不到音节是错
    let mut chunks: Vec<(&str, Option<u8>)> = Vec::new();
    let mut start = 0;
    for (index, (position, ch)) in input.char_indices().enumerate() {
        let char_position = index + 1;
        if ch.is_ascii_lowercase() {
            continue;
        }
        if ch == '\'' || is_tone_mark(ch).is_some() {
            if ch == '\'' {
                if position > start {
                    chunks.push((&input[start..position], None));
                }
            } else if position > start {
                chunks.push((&input[start..position], is_tone_mark(ch)));
            } else {
                return Err(ParseError::InvalidChar {
                    position: char_position,
                    ch,
                });
            }
            start = position + ch.len_utf8();
            continue;
        }
        return Err(ParseError::InvalidChar {
            position: char_position,
            ch,
        });
    }
    if start < input.len() {
        chunks.push((&input[start..], None));
    }
    if chunks.is_empty() {
        return Err(ParseError::NoSegmentation);
    }
    let mut results: Vec<Segmentation> = vec![Segmentation {
        syllables: Vec::new(),
    }];
    for (index, (chunk, tone)) in chunks.iter().enumerate() {
        let is_last = index + 1 == chunks.len();
        let options = segment_chunk(chunk, is_last);
        if options.is_empty() {
            return Err(ParseError::NoSegmentation);
        }
        let mut next = Vec::with_capacity(results.len() * options.len());
        for base in &results {
            for option in &options {
                let mut syllables = base.syllables.clone();
                syllables.extend(option.syllables.iter().cloned());
                if let Some(tone) = tone
                    && let Some(last) = syllables.last_mut()
                {
                    last.tone = Some(*tone);
                }
                next.push(Segmentation { syllables });
            }
        }
        results = next;
    }
    prune(&mut results);
    Ok(results)
}

/// 排序并截断到 [`MAX_SEGMENTATIONS`]。
fn prune(segmentations: &mut Vec<Segmentation>) {
    segmentations.sort_by_cached_key(sort_key);
    segmentations.dedup();
    segmentations.truncate(MAX_SEGMENTATIONS);
}

/// 音节少的优先；同音节数时不完整音节少的优先；再同则前面的音节越长越优先（贪心的结果排最前）。
fn sort_key(segmentation: &Segmentation) -> (usize, usize, Vec<Reverse<usize>>) {
    (
        segmentation.syllables.len(),
        segmentation.incomplete_count(),
        segmentation
            .syllables
            .iter()
            .map(|s| Reverse(s.text.len()))
            .collect(),
    )
}

/// 切分不含 `'` 的一段：按位置做动态规划，每个位置只保留最优的几种前缀切分。
/// `allow_partial` 为真时末尾允许留一个残缺音节。
fn segment_chunk(chunk: &str, allow_partial: bool) -> Vec<Segmentation> {
    let n = chunk.len();
    let mut best: Vec<Vec<Segmentation>> = vec![Vec::new(); n + 1];
    best[0].push(Segmentation {
        syllables: Vec::new(),
    });
    for start in 0..n {
        if best[start].is_empty() {
            continue;
        }
        prune(&mut best[start]);
        let rest = &chunk[start..];
        let matches = SYLLABLE_TRIE.matches(rest);
        // (长度, 是否完整音节)
        let mut tokens: Vec<(usize, bool)> = matches.lengths().map(|len| (len, true)).collect();
        for len in initial_lengths(rest) {
            if !tokens.contains(&(len, true)) {
                tokens.push((len, false));
            }
        }
        // 整个剩余部分作为未打完的音节（`zho` → zhong / zhou …）。它本身是完整音节或纯声母时已经在上面了。
        let rest_is_complete = matches.lengths().next_back() == Some(rest.len());
        if allow_partial
            && matches.whole_is_prefix
            && !rest_is_complete
            && !tokens.contains(&(rest.len(), false))
        {
            tokens.push((rest.len(), false));
        }
        for (len, complete) in tokens {
            let text = &rest[..len];
            let extended: Vec<Segmentation> = best[start]
                .iter()
                .map(|base| {
                    let mut syllables = base.syllables.clone();
                    syllables.push(if complete {
                        Syllable::complete(text)
                    } else {
                        Syllable::partial(text)
                    });
                    Segmentation { syllables }
                })
                .collect();
            best[start + len].extend(extended);
        }
    }
    let mut result = std::mem::take(&mut best[n]);
    prune(&mut result);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(input: &str) -> Vec<String> {
        segment(input)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn greedy_segmentation_comes_first() {
        assert_eq!(joined("kaifa")[0], "kai fa");
        assert_eq!(joined("zhongwen")[0], "zhong wen");
    }

    #[test]
    fn ambiguous_input_yields_multiple_segmentations() {
        let all = joined("xian");
        assert_eq!(all[0], "xian");
        assert!(all.contains(&"xi an".to_owned()));
    }

    #[test]
    fn apostrophe_forces_boundary() {
        let all = joined("xi'an");
        assert_eq!(all[0], "xi an");
        assert!(!all.iter().any(|s| s.starts_with("xian")));
    }

    #[test]
    fn trailing_partial_syllable_is_marked() {
        assert_eq!(joined("kaif")[0], "kai f…");
        assert_eq!(joined("zho")[0], "zho…");
    }

    #[test]
    fn initials_only_segment_as_abbreviations() {
        assert_eq!(joined("kf")[0], "k… f…");
        assert_eq!(joined("zhw")[0], "zh… w…");
        assert!(joined("zhw").contains(&"z… h… w…".to_owned()));
    }

    #[test]
    fn mixed_full_and_abbreviated_syllables() {
        assert_eq!(joined("kfa")[0], "k… fa");
        assert_eq!(joined("kaif")[0], "kai f…");
        assert_eq!(joined("srf")[0], "s… r… f…");
    }

    #[test]
    fn complete_segmentation_ranks_before_partial() {
        let all = segment("xia").unwrap();
        assert_eq!(all[0].syllables, [Syllable::complete("xia")]);
    }

    #[test]
    fn long_input_stays_bounded() {
        let all = segment("womenjintianxiawuqukaihuiba").unwrap();
        assert!(all.len() <= MAX_SEGMENTATIONS);
        assert_eq!(all[0].to_string(), "wo men jin tian xia wu qu kai hui ba");
    }

    #[test]
    fn full_segmentability_agrees_with_segment() {
        for text in ["nihao", "zhongguo", "xian", "a", "kaifazhe"] {
            assert!(is_fully_segmentable(text), "{text}");
            assert_eq!(segment(text).unwrap()[0].incomplete_count(), 0, "{text}");
        }
        for text in ["", "nih", "kaif", "zhzh", "v", "nihoa"] {
            assert!(!is_fully_segmentable(text), "{text}");
        }
    }

    #[test]
    fn rejects_invalid_input() {
        assert_eq!(segment("").unwrap_err(), ParseError::Empty);
        assert_eq!(
            segment("kai1").unwrap_err(),
            ParseError::InvalidChar {
                position: 4,
                ch: '1'
            }
        );
        assert_eq!(segment("v").unwrap_err(), ParseError::NoSegmentation);
        assert_eq!(segment("kaiv").unwrap_err(), ParseError::NoSegmentation);
    }

    #[test]
    fn tone_marks_attach_to_the_preceding_syllable() {
        let all = joined("ni-hao.");
        assert_eq!(all[0], "ni- hao.");
        let tones: Vec<Option<u8>> = segment("ni-hao.").unwrap()[0]
            .syllables
            .iter()
            .map(|s| s.tone)
            .collect();
        assert_eq!(tones, [Some(1), Some(5)]);
        // 调号挂在段的最后一个音节上
        let tones: Vec<Option<u8>> = segment("nihao-").unwrap()[0]
            .syllables
            .iter()
            .map(|s| s.tone)
            .collect();
        assert_eq!(tones, [None, Some(1)]);
        // 只有调号的段与 `'` 一样不参与
        assert_eq!(joined("ni'hao")[0], "ni hao");
    }

    #[test]
    fn stray_tone_marks_are_invalid() {
        assert_eq!(
            segment("-ni").unwrap_err(),
            ParseError::InvalidChar {
                position: 1,
                ch: '-'
            }
        );
        assert_eq!(
            segment("ni--hao").unwrap_err(),
            ParseError::InvalidChar {
                position: 4,
                ch: '-'
            }
        );
        // 与 `'` 一样，开头的空段忽略，`'ni` 照旧能切
        assert_eq!(joined("'ni")[0], "ni");
    }

    #[test]
    fn tone_keys_are_parsed_without_changing_letters() {
        let segmentation = &segment("xi'an=").unwrap()[0];
        assert_eq!(segmentation.to_string(), "xi an=");
        assert_eq!(segmentation.letters(), 4);
        assert_eq!(segmentation.syllables[1].tone, Some(3));
    }
}
