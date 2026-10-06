//! 声调表：词库 + CC-CEDICT（词级带调读音）+ Unihan（逐字读音兜底）→ `tone.qj`。
//!
//! 这是「声调参与候选匹配」的运行时旁表（设计见 `docs/design/pinyin-tone.md`）：
//!
//! - key 是 `词\t词库拼音`（与运行时 `"{hit.text}\t{hit.pinyin}"` 完全一致，词目与读音都按词库原样）；
//! - code 按音节逐位给字母编码：`a`=一声 `b`=二声 `c`=三声 `d`=四声 `e`=轻声 `o`=未知（旁表拿不准就放行），
//!   音节数不能超过 8（`AuxCodeTable::MAX_CODE_LEN`），超长词跳过；
//! - 同一个 key 可以有多个码（多音词的每个读音一条），运行时任一读音对上就保留。
//!
//! 读音来源两步走：CC-CEDICT 按词整体匹配（`重新` = `chong2 xin1` 逐音节对得上才用）；
//! 对不上的词退回 Unihan 逐字查（`kMandarin` / `kHanyuPinlu` / `kXHC1983`，多个读音对不上时该位记 `o`）。
//! 两处都没有把握的词整词不进表——旁表只收紧、不放逐，运行时没码的词照常出候选。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use qingjian_dictionary::{AuxCodeTable, Dictionary};
use qingjian_format::Metadata;

use crate::error::ConvertError;
use crate::lexicon::tone::strip_tone;

#[cfg(test)]
mod tests;

/// 音节数上限：码是逐音节一个字母，`AuxCodeTable` 的码长上限是 8。
const MAX_SYLLABLES: usize = 8;

/// 调 → 码字（`is_tone_mark` 的 1–5 加未知 `o`）。
const fn tone_letter(tone: u8) -> char {
    match tone {
        1 => 'a',
        2 => 'b',
        3 => 'c',
        4 => 'd',
        5 => 'e',
        _ => 'o',
    }
}

/// 简体词 → 读音列表；每个读音是逐音节的 `(无声调基底, 调 1–5)`。
type Readings = HashMap<String, Vec<Vec<(String, u8)>>>;

/// 逐字读音：字 → `(基底, 调)` 列表。
type CharReadings = HashMap<char, Vec<(String, u8)>>;

/// 生成结果的统计（日志与测试断言用）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ToneStats {
    /// 扫过的词条数（词库条数）。
    pub entries: usize,
    /// 去重后的「词 + 读音」数。
    pub words: usize,
    /// 至少一个读音有码的词数。
    pub coded: usize,
    /// 读音由 CC-CEDICT 词级对上的词数。
    pub via_cedict: usize,
    /// 读音由 Unihan 逐字兜底的词数。
    pub via_unihan: usize,
    /// 有多个读音码的词数（多音词）。
    pub multi_reading: usize,
    /// 字数与音节数对不上（英文 / 混排 / 切分不齐）而跳过。
    pub skipped_align: usize,
    /// 音节数超过上限而跳过。
    pub skipped_too_long: usize,
    /// 两处来源都拿不准读音而跳过。
    pub skipped_unknown: usize,
}

/// 读 dict.qj + CEDICT + Unihan，生成 `tone.qj`。
pub fn build(
    dict: &Path,
    cedict_path: &Path,
    unihan_path: &Path,
    out: &Path,
    metadata: &Metadata,
) -> Result<ToneStats, ConvertError> {
    let started = Instant::now();
    let cedict = load_cedict(cedict_path)?;
    tracing::info!(path = %cedict_path.display(), words = cedict.len(), "已读 CC-CEDICT 读音");
    let unihan = load_unihan(unihan_path)?;
    tracing::info!(path = %unihan_path.display(), chars = unihan.len(), "已读 Unihan 读音");
    let dictionary = Dictionary::from_path(dict)?;
    let (pairs, stats) = collect(&dictionary, &cedict, &unihan);
    if stats.words == 0 {
        tracing::warn!(path = %dict.display(), "词库是空的：声调表不会有条目");
    }
    let table = AuxCodeTable::from_pairs(pairs)?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    table.write_qj(out, metadata)?;

    let size = std::fs::metadata(out).map(|meta| meta.len()).unwrap_or(0);
    tracing::info!(
        out = %out.display(),
        dict = %dict.display(),
        dict_entries = stats.entries,
        words = stats.words,
        coded = stats.coded,
        via_cedict = stats.via_cedict,
        via_unihan = stats.via_unihan,
        multi_reading = stats.multi_reading,
        skipped_align = stats.skipped_align,
        skipped_too_long = stats.skipped_too_long,
        skipped_unknown = stats.skipped_unknown,
        entries = table.len(),
        size_kb = size / 1_000,
        elapsed_ms = started.elapsed().as_millis(),
        "已生成声调表"
    );
    Ok(stats)
}

/// 词库遍历 → `(key, 码)` 对（一 key 多码）与统计。
fn collect(
    dictionary: &Dictionary,
    cedict: &Readings,
    unihan: &CharReadings,
) -> (Vec<(String, String)>, ToneStats) {
    let mut stats = ToneStats {
        entries: dictionary.len(),
        ..ToneStats::default()
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut pairs = Vec::new();
    for entry in dictionary.entries() {
        let syllables: Vec<&str> = entry.pinyin.split(' ').collect();
        if syllables.len() > MAX_SYLLABLES {
            stats.skipped_too_long += 1;
            continue;
        }
        let chars: Vec<char> = entry.text.chars().collect();
        if chars.len() != syllables.len() {
            stats.skipped_align += 1;
            continue;
        }
        let key = format!("{}\t{}", entry.text, entry.pinyin);
        if !seen.insert(key.clone()) {
            continue;
        }
        stats.words += 1;
        let mut codes: Vec<String> = Vec::new();
        // CEDICT 词级：整词读音逐音节对得上才用
        if let Some(readings) = cedict.get(entry.text) {
            for reading in readings {
                if reading.len() == syllables.len()
                    && reading
                        .iter()
                        .zip(&syllables)
                        .all(|((base, _), syllable)| base == syllable)
                {
                    let code: String = reading.iter().map(|(_, tone)| tone_letter(*tone)).collect();
                    if !codes.contains(&code) {
                        codes.push(code);
                    }
                }
            }
        }
        let via_cedict = !codes.is_empty();
        // Unihan 逐字兜底：每个字按基底挑读音，挑不准（多个 / 没有）的位记 o
        if codes.is_empty() {
            let mut any_known = false;
            let mut code = String::with_capacity(chars.len());
            for (ch, syllable) in chars.iter().zip(&syllables) {
                let tones: HashSet<u8> = unihan
                    .get(ch)
                    .map(|readings| {
                        readings
                            .iter()
                            .filter(|(base, _)| base == syllable)
                            .map(|(_, tone)| *tone)
                            .collect()
                    })
                    .unwrap_or_default();
                match tones.len() {
                    1 => {
                        any_known = true;
                        code.push(tone_letter(*tones.iter().next().expect("set len is 1")));
                    }
                    _ => code.push('o'),
                }
            }
            if any_known {
                codes.push(code);
            }
        }
        if codes.is_empty() {
            stats.skipped_unknown += 1;
            continue;
        }
        stats.coded += 1;
        if via_cedict {
            stats.via_cedict += 1;
        } else {
            stats.via_unihan += 1;
        }
        if codes.len() > 1 {
            stats.multi_reading += 1;
        }
        for code in codes {
            pairs.push((key.clone(), code));
        }
    }
    (pairs, stats)
}

/// 读 CC-CEDICT：`繁体 简体 [拼音] /释义…`，按简体合并读音；坏行整行跳过（只是旁表，不值得中断）。
fn load_cedict(path: &Path) -> Result<Readings, ConvertError> {
    let source = std::fs::read_to_string(path)?;
    let mut map: Readings = HashMap::new();
    for line in source.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((head, rest)) = line.split_once(" [") else {
            continue;
        };
        let Some(simplified) = head.split_whitespace().next_back() else {
            continue;
        };
        let Some((raw_pinyin, _)) = rest.split_once(']') else {
            continue;
        };
        if let Some(reading) = parse_reading(raw_pinyin) {
            map.entry(simplified.to_owned()).or_default().push(reading);
        }
    }
    Ok(map)
}

/// `zhong1 guo2` → `[("zhong", 1), ("guo", 2)]`；任一音节不合规整条作废。
fn parse_reading(raw: &str) -> Option<Vec<(String, u8)>> {
    let mut reading = Vec::new();
    for syllable in raw.split_whitespace() {
        reading.push(parse_syllable(syllable)?);
    }
    (!reading.is_empty()).then_some(reading)
}

/// `zhong1` → `("zhong", 1)`；CEDICT 的 `u:` 写法归一成 v（`lu:4` → `lv`）。
fn parse_syllable(raw: &str) -> Option<(String, u8)> {
    let last = raw.chars().last()?;
    let tone = u8::try_from(last.to_digit(10)?).ok()?;
    if !(1..=5).contains(&tone) {
        return None;
    }
    let head = &raw[..raw.len() - last.len_utf8()];
    let normalized = head.to_ascii_lowercase().replace("u:", "v");
    Some((strip_tone(&normalized), tone))
}

/// 读 Unihan：`kMandarin` / `kHanyuPinlu` / `kXHC1983` 的带调读音 → 逐字 `(基底, 调)`。
fn load_unihan(path: &Path) -> Result<CharReadings, ConvertError> {
    let source = std::fs::read_to_string(path)?;
    let mut map: CharReadings = HashMap::new();
    for line in source.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(codepoint), Some(field), Some(value)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Some(ch) = codepoint
            .strip_prefix("U+")
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .and_then(char::from_u32)
        else {
            continue;
        };
        for raw in extract_readings(field, value) {
            if let Some(reading) = parse_unihan_syllable(&raw) {
                let entry = map.entry(ch).or_default();
                if !entry.contains(&reading) {
                    entry.push(reading);
                }
            }
        }
    }
    Ok(map)
}

/// 三个字段各取哪段当读音（与 `lexicon::readings` 同口径，这里保留调号）。
fn extract_readings(field: &str, value: &str) -> Vec<String> {
    match field {
        // 只取第一个候选（Unihan 排序：简体读音优先），其余多半是异读
        "kMandarin" => value
            .split_whitespace()
            .take(1)
            .map(str::to_owned)
            .collect(),
        // `xíng(2943) háng(132)` → 括号里是词频
        "kHanyuPinlu" => value
            .split_whitespace()
            .filter_map(|item| item.split_once('(').map(|(reading, _)| reading.to_owned()))
            .collect(),
        // `0442.080:háng 1008.120:xíng`
        "kXHC1983" => value
            .split_whitespace()
            .filter_map(|item| item.split_once(':').map(|(_, reading)| reading.to_owned()))
            .collect(),
        _ => Vec::new(),
    }
}

/// `xíng` → `("xing", 1)`；调号按音节里的调符判定，拿不准（复合元音 / 冲突）就不认这条。
fn parse_unihan_syllable(raw: &str) -> Option<(String, u8)> {
    let tone = tone_of(raw)?;
    Some((strip_tone(raw), tone))
}

/// 读音的调：调符（预组合或组合用 U+0300–U+0304）给出 1–4；都没有但有裸元音则当轻声 5；两个调冲突就不认。
fn tone_of(reading: &str) -> Option<u8> {
    let mut tone: Option<u8> = None;
    let mut plain_vowel = false;
    for c in reading.chars() {
        if let Some(found) = mark_tone(c) {
            match tone {
                None => tone = Some(found),
                Some(existing) if existing == found => {}
                Some(_) => return None,
            }
        } else if matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'v' | 'ü' | 'ê') {
            plain_vowel = true;
        }
    }
    tone.or_else(|| plain_vowel.then_some(5))
}

/// 单个字符的调（1–4）；不认识的字符没有调。
const fn mark_tone(c: char) -> Option<u8> {
    match c {
        'ā' | 'ē' | 'ī' | 'ō' | 'ū' | 'ǖ' => Some(1),
        'á' | 'é' | 'í' | 'ó' | 'ú' | 'ǘ' | 'ń' | 'ḿ' => Some(2),
        'ǎ' | 'ě' | 'ǐ' | 'ǒ' | 'ǔ' | 'ǚ' | 'ň' => Some(3),
        'à' | 'è' | 'ì' | 'ò' | 'ù' | 'ǜ' | 'ǹ' => Some(4),
        '\u{0304}' => Some(1),
        '\u{0301}' => Some(2),
        '\u{030c}' => Some(3),
        '\u{0300}' => Some(4),
        _ => None,
    }
}
