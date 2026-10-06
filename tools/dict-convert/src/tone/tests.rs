//! 声调表的读音解析、取码规则与整管线生成。

use std::path::{Path, PathBuf};

use qingjian_format::Metadata;

use crate::tone::{ToneStats, build, extract_readings, parse_reading, parse_syllable, tone_of};

/// 一次测试一个目录，结束删掉。
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("qingjian-tone-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// 写一个测试用的输入文件。
    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// CEDICT 数字调、u: 归一、坏音节整条作废。
#[test]
fn parses_cedict_syllables() {
    assert_eq!(parse_syllable("zhong1"), Some(("zhong".to_owned(), 1)));
    assert_eq!(parse_syllable("guo2"), Some(("guo".to_owned(), 2)));
    assert_eq!(parse_syllable("lu:4"), Some(("lv".to_owned(), 4)));
    assert_eq!(parse_syllable("lue4"), Some(("lve".to_owned(), 4)));
    assert_eq!(parse_syllable("de5"), Some(("de".to_owned(), 5)));
    assert_eq!(parse_syllable("de"), None); // 没有数字调
    assert_eq!(parse_syllable("zhong9"), None); // 只认 1–5
    assert_eq!(parse_syllable(""), None);

    let reading = parse_reading("chong2 xin1").unwrap();
    assert_eq!(
        reading,
        vec![("chong".to_owned(), 2), ("xin".to_owned(), 1)]
    );
    assert!(parse_reading("xin1 lian2 zai3").is_some());
    assert!(parse_reading("").is_none());
    assert!(parse_reading("xin1 oops").is_none()); // 任一音节坏了整条作废
}

/// Unihan 带调读音：调符给 1–4，裸元音当轻声，冲突不认。
#[test]
fn reads_tones_from_unihan_marks() {
    assert_eq!(tone_of("xíng"), Some(2));
    assert_eq!(tone_of("háng"), Some(2));
    assert_eq!(tone_of("kǎ"), Some(3));
    assert_eq!(tone_of("zhòng"), Some(4));
    assert_eq!(tone_of("de"), Some(5)); // 没调符的轻声
    assert_eq!(tone_of("a"), Some(5));
    assert_eq!(tone_of("a\u{0304}"), Some(1)); // 组合用调符
    assert_eq!(tone_of("a\u{030c}"), Some(3));
    assert_eq!(tone_of("ń"), Some(2));
    assert_eq!(tone_of("ángā"), None); // 两个调冲突
    assert_eq!(tone_of("ng"), None); // 没有元音也没有调
}

/// 三个 Unihan 字段各取哪段。
#[test]
fn extracts_readings_per_field() {
    assert_eq!(extract_readings("kMandarin", "xíng háng"), ["xíng"]);
    assert_eq!(
        extract_readings("kHanyuPinlu", "xíng(2943) háng(132)"),
        ["xíng", "háng"]
    );
    assert_eq!(
        extract_readings("kXHC1983", "0442.080:háng 1008.120:xíng"),
        ["háng", "xíng"]
    );
    assert!(extract_readings("kDefinition", "to walk").is_empty());
}

/// 整管线：词库 + CEDICT + Unihan → `tone.qj`，含词级优先、逐字兜底与三类跳过。
#[test]
fn build_writes_tone_table_for_the_whole_dictionary() {
    let dir = TempDir::new("build");
    let dict = dir.write(
        "dict.tsv",
        "明明\tming ming\t100\n你好\tni hao\t90\n得\tde\t80\n一\tyi\t70\n喵\tmiao\t60\nhello\thello\t50\n一二三四五六七八九\tyi er san si wu liu qi ba jiu\t40\n",
    );
    let cedict = dir.write(
        "cedict.txt",
        "# comment\n明明 明明 [ming2 ming2] : clearly\n得 得 [de2] : to get\n得 得 [de5] : particle\n坏行没有方括号\n",
    );
    let unihan = dir.write(
        "unihan.txt",
        "# comment\nU+4F60\tkMandarin\tnǐ\nU+597D\tkMandarin\thǎo\nU+4E00\tkMandarin\tyī\nU+660E\tkMandarin\tmíng\n",
    );
    let out = dir.path().join("tone.qj");
    let metadata = Metadata {
        name: "声调".to_owned(),
        license: "CC-BY-SA-4.0".to_owned(),
        ..Metadata::default()
    };

    let stats = build(&dict, &cedict, &unihan, &out, &metadata).unwrap();
    assert_eq!(stats.entries, 7);
    assert_eq!(stats.words, 5); // hello 与九音节在记词前就跳过了
    // 词级 CEDICT 对上的：明明（bb）、得（b / e 两个读音）
    assert_eq!(stats.via_cedict, 2);
    assert_eq!(stats.multi_reading, 1);
    // 逐字兜底：你好（nǐ hǎo → cc）、一（yī → a）
    assert_eq!(stats.via_unihan, 2);
    assert_eq!(stats.coded, 4);
    assert_eq!(stats.skipped_unknown, 1); // 喵：两处都没有读音
    assert_eq!(stats.skipped_align, 1); // hello：字数与音节数对不上
    assert_eq!(stats.skipped_too_long, 1); // 九音节超过 8

    let table = qingjian_dictionary::AuxCodeTable::open(&out).unwrap();
    assert_eq!(table.len(), 5); // 4 个词 + 得的第二个读音码
    assert_eq!(table.word_count(), 4);
    assert_eq!(table.metadata().unwrap().name, "声调");
    assert_eq!(table.metadata().unwrap().license, "CC-BY-SA-4.0");
    // 词级 CEDICT 优先，不看 Unihan
    assert_eq!(
        table.codes_of("明明\tming ming").collect::<Vec<_>>(),
        ["bb"]
    );
    // 多音词一 key 多码，运行时任一读音对上就保留
    assert_eq!(table.codes_of("得\tde").collect::<Vec<_>>(), ["b", "e"]);
    // 逐字兜底逐位成码
    assert_eq!(table.codes_of("你好\tni hao").collect::<Vec<_>>(), ["cc"]);
    assert_eq!(table.codes_of("一\tyi").collect::<Vec<_>>(), ["a"]);
    assert_eq!(table.codes_of("喵\tmiao").count(), 0);
}

/// 空词库：产物是空表，统计全 0，不报错。
#[test]
fn empty_dictionary_produces_an_empty_table() {
    let dir = TempDir::new("empty");
    let dict = dir.write("dict.tsv", "");
    let cedict = dir.write("cedict.txt", "");
    let unihan = dir.write("unihan.txt", "");
    let out = dir.path().join("tone.qj");
    let metadata = Metadata {
        name: "声调".to_owned(),
        ..Metadata::default()
    };

    let stats = build(&dict, &cedict, &unihan, &out, &metadata).unwrap();
    assert_eq!(stats, ToneStats::default());

    let table = qingjian_dictionary::AuxCodeTable::open(&out).unwrap();
    assert!(table.is_empty());
    assert_eq!(table.word_count(), 0);
}
