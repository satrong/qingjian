//! 声调匹配：调号内联切分、旁表硬过滤、零命中降级、raw 边界与双拼路径。

use qingjian_dictionary::AuxCodeTable;

use super::*;

/// 样例词库：知道（1+4）与指导（3+3）同一个输入、调不同，一个筛掉一个留下。
const TONE_SAMPLE: &str = "知道\tzhi dao\t5000\n指导\tzhi dao\t4000\n知\tzhi\t3000\n指\tzhi\t1500\n道\tdao\t2000\n导\tdao\t1000\n开\tkai\t6000\n";

/// 样例词库的声调旁表（`a`=1 … `e`=5）。
fn tones() -> Arc<AuxCodeTable> {
    Arc::new(
        AuxCodeTable::from_pairs([
            // 旁表键与运行时一致：`词\t词库拼音`
            ("知道\tzhi dao".to_owned(), "ad".to_owned()), // zhī dào
            ("指导\tzhi dao".to_owned(), "cc".to_owned()), // zhǐ dǎo
            ("知\tzhi".to_owned(), "a".to_owned()),
            ("指\tzhi".to_owned(), "c".to_owned()),
            ("道\tdao".to_owned(), "d".to_owned()),
            ("导\tdao".to_owned(), "c".to_owned()),
            ("开\tkai".to_owned(), "a".to_owned()), // kāi
        ])
        .unwrap(),
    )
}

fn tone_engine() -> Engine {
    let mut engine = Engine::new(Dictionary::parse(TONE_SAMPLE).unwrap());
    engine.set_tone_matching(true);
    engine.set_tone_table(Some(tones()));
    engine
}

fn candidates(engine: &mut Engine, input: &str) -> Vec<String> {
    engine.set_input(input);
    engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .map(|c| c.text)
        .collect()
}

/// 调号挂在音节上当调：`zhidao=`（三声）只留指导（3+3），知道（1+4）被筛掉；
/// 单字前缀候选不带调、不受影响。
#[test]
fn typed_tones_filter_candidates_by_the_table() {
    let mut engine = tone_engine();
    let items = candidates(&mut engine, "zhidao=");
    assert!(items.contains(&"指导".to_owned()));
    assert!(!items.contains(&"知道".to_owned()));
    assert!(items.contains(&"知".to_owned()));
    assert!(items.contains(&"指".to_owned()));
}

/// 调号对上任一读音即可；没敲调号时旁表不参与（列表与不开声调匹配时一致）。
#[test]
fn without_any_tone_marks_the_table_stays_out_of_the_way() {
    let mut engine = tone_engine();
    let plain = candidates(&mut engine, "zhidao");
    let mut bare = Engine::new(Dictionary::parse(TONE_SAMPLE).unwrap());
    assert_eq!(plain, candidates(&mut bare, "zhidao"));
    assert!(plain.contains(&"知道".to_owned()));
    assert!(plain.contains(&"指导".to_owned()));
}

/// 全部词都被调号筛掉时退回无声调的结果：调号打错不能把输入打空。
#[test]
fn zero_hit_tones_degrade_back_to_unttoned_results() {
    let mut engine = tone_engine();
    let rejected = candidates(&mut engine, "kai="); // kāi 要一声，三声无词
    let plain = candidates(&mut engine, "kai");
    assert_eq!(rejected, plain);
    assert!(rejected.contains(&"开".to_owned()));

    // 没装旁表时调号照常是拼音键，但不筛词
    let mut no_table = Engine::new(Dictionary::parse(TONE_SAMPLE).unwrap());
    no_table.set_tone_matching(true);
    assert_eq!(
        candidates(&mut no_table, "zhidao="),
        candidates(&mut no_table, "zhidao")
    );

    // 开关关着时维持英文直输段的老判定：`-` 是外来字符
    let mut off = Engine::new(Dictionary::parse(TONE_SAMPLE).unwrap());
    off.set_input("kai-fan");
    assert!(off.raw_mode());
}

/// raw 边界：调号在开着时算拼音键（`kai-fan` 不再直输），坏输入（开头调号、无音节可切）
/// 仍然原样上屏；开关关着一切照旧。
#[test]
fn raw_mode_respects_the_tone_switch() {
    let mut engine = tone_engine();

    engine.set_input("kai-fan");
    assert!(!engine.raw_mode());
    engine.set_input("no-way");
    assert!(engine.raw_mode());
    engine.set_input("-kai");
    assert!(engine.raw_mode()); // 开头的调号挂不上音节
    engine.set_input("kai-");
    assert!(!engine.raw_mode()); // 连着敲的第二个调号改调，不成坏输入
    engine.set_input("kai'");
    assert!(!engine.raw_mode()); // 撇号照旧只是音节分隔

    let mut off = Engine::new(Dictionary::parse(TONE_SAMPLE).unwrap());
    off.set_input("kai-fan");
    assert!(off.raw_mode()); // 老行为：`-` 是外来字符
    off.set_input("a.b");
    assert!(off.raw_mode());
    off.set_input("no-way");
    assert!(off.raw_mode());
}

/// 双拼：调号跟在音节键后进 `unit.keys`（上屏消耗得住），过滤与全拼同一条路。
#[test]
fn shuangpin_tones_attach_to_units_and_filter() {
    let mut engine = tone_engine();
    engine.set_shuangpin(Some(Scheme::Xiaohe));
    // 小鹤：zhi = vi、dao = dc、zhidao = vidc；三声挂最后
    engine.set_input("vidc=");
    let items = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .map(|c| c.text)
        .collect::<Vec<_>>();
    assert!(items.contains(&"指导".to_owned()));
    assert!(!items.contains(&"知道".to_owned()));

    // 双拼里开头的调号没有音节可挂 → 原样上屏
    engine.set_input("=vidc");
    assert!(engine.raw_mode());
}

/// 注音：调号是布局键、进 `Syllable.tone`，但不参与「外来字符」判定（`tone && !zhuyin`）。
#[test]
fn zhuyin_keeps_its_own_tone_keys_out_of_the_raw_gate() {
    let mut engine = tone_engine();
    engine.set_zhuyin_mode(true);
    engine.set_input("1j4"); // 已有用例里的注音键串，带调号键
    assert!(!engine.raw_mode());
}

/// 调号要出现在拼音串里：输入什么调号就显示什么调号，用户才看得出自己标的是哪一声。
#[test]
fn tone_marks_show_up_in_the_preedit() {
    let mut engine = tone_engine();
    for (input, expected) in [
        ("ni=", "ni="),
        ("ni-", "ni-"),
        ("ni-hao", "ni-'hao"),
        ("nihao=", "ni'hao="),
        ("kai-fan.", "kai-'fan."),
    ] {
        engine.set_input(input);
        let query = engine.query().unwrap();
        assert_eq!(query.marked_text(), expected, "{input}");
    }
}

/// 光标停在调号前后都要落在显示串的对应位置（显示串里调号占一格）。
#[test]
fn cursor_maps_onto_the_tone_marks() {
    let mut engine = tone_engine();
    engine.set_input("ni-hao");
    let at = |engine: &mut Engine, moves: usize| {
        engine.move_cursor_home();
        for _ in 0..moves {
            engine.move_cursor_right();
        }
        let query = engine.query().unwrap();
        (query.marked_text(), query.marked_cursor())
    };
    assert_eq!(at(&mut engine, 0), ("ni-'hao".to_owned(), 0));
    // 光标在中间时作用域只有光标前那段，`-` 归后面的 hao，跟着剩余拼音显示
    assert_eq!(at(&mut engine, 2), ("ni'-hao".to_owned(), 2)); // ni|
    assert_eq!(at(&mut engine, 3), ("ni-'hao".to_owned(), 3)); // ni-|
    // 末尾：显示串比敲的多一个自动补的 `'`
    assert_eq!(at(&mut engine, 6), ("ni-'hao".to_owned(), 7));
}

/// 一个音节后只留一个调号：连着敲第二个调号是改调，不是追加第二个。
/// `ni-=` → `ni=`，退格一次就回到 `ni`。
#[test]
fn a_second_tone_mark_replaces_the_first() {
    let mut engine = tone_engine();
    engine.set_input("ni-=");
    assert!(!engine.raw_mode());
    assert_eq!(engine.composition().text(), "ni=");
    let query = engine.query().unwrap();
    assert_eq!(query.marked_text(), "ni=");
    assert!(engine.backspace());
    assert_eq!(engine.composition().text(), "ni");
    // 每个音节各留一个：`ni-hao=` → `ni-hao=`；连续敲只改最后那一个
    engine.set_input("ni-hao-=");
    assert_eq!(engine.composition().text(), "ni-hao=");
    assert!(!engine.raw_mode());
}

/// 改调之后筛词也跟着走：`zhi-` 留知（一声），补一个 `=` 改成三声就只剩指。
#[test]
fn switching_tone_switches_the_candidate_filter() {
    let mut engine = tone_engine();
    assert!(candidates(&mut engine, "zhi-").contains(&"知".to_owned()));
    let items = candidates(&mut engine, "zhi-=");
    assert!(items.contains(&"指".to_owned()));
    assert!(!items.contains(&"知".to_owned()));
}
