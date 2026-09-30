//! 提示词与回复解析。模型只输出约定的 JSON，其余一概不信；拼音校验在 Core 里再做一遍。

use qingjian_core::{CloudWord, PredictionKind, PredictionRequest};
use serde::{Deserialize, Serialize};

/// 组句联想的系统提示。words 的硬性要求照抄 Core 的 `validate_cloud_words` 与 `fuzzy::tolerance`、
/// 候选窗的同文去重，那边改了这里要跟着改，否则模型给的词会被悄悄丢掉。
pub const SYSTEM_PROMPT: &str = r#"你是拼音输入法的云端联想引擎。用户正在敲一段拼音、还没选词，你要猜出他想打的字，补上本地词库给不出的候选。

输入是一段 JSON：
- letters：用户实际敲的字母。可能有错字、漏字、多字；可能是简拼（单个字母是声母缩写）；最后一个音节可能还没敲完。
- pinyin：本地对 letters 的切分，' 分隔，切不动的尾巴原样接在最后，切分可能是错的；syllables 是它的音节数，仅供参考。
- before / after：应用里光标前后的文本，应用给不出时为空。你给的字会插在两者之间。
- local_candidates：本地词库排在前面的候选，第一个是本地首选；local_sentence：本地整句转换的结果，可能为空。两者都可能是错的。
- max_items：words 最多几条；want_sentence：要不要 sentence。

输出 JSON：{"words": [{"text": "…", "pinyin": "…"}], "sentence": "…" 或 null}

words：用户最可能想打、而 local_candidates 里没有的词或短语，按可能性从高到低排，只有最前面一两条会显示出来。
下面是硬性要求，不满足的会被输入法直接丢掉：
- 每条都要对应 letters 的**全部**字母，从第一个到最后一个，不能只对应开头一段；
- pinyin 是 text 的标准全拼：不带声调，音节间用空格，ü 写作 v（lv、nve）；text 的字数等于 pinyin 的音节数；
- 每个音节在 letters 里可以只敲了开头几个字母（简拼、没敲完），这不算错；真正敲错、多敲、漏敲的字母：\
letters 不到 4 个时一处都不许有，4 到 9 个最多 1 处，10 到 15 个最多 2 处，再长每 6 个字母多容 1 处；
- 与 local_candidates 中任何一条相同的不要给；
- 用简体中文。

怎么猜：
- 先看 before / after 是什么领域（财务、编程、医学、日常聊天……），同音词里挑这个领域说得通的；
- 你的价值在本地词库缺的东西：术语、新词、人名、机构名、产品名；本地首选不合上下文时给出对的那个；letters 有明显错拼时给纠正后的词（kaufa → 开发）；
- 只给真实存在、在这里说得通的词：不要同音生造（不态、步太），不要按声母硬凑（fhyq 凑成 复合语气）；
- 本地首选已经合适、又想不出更好的，就给空数组；不确定就少给，不要凑数；
- max_items 为 0 时 words 给空数组。

sentence：want_sentence 为 false 时给 null。为 true 时给一条用户最可能要上屏的完整短句：\
开头就是 letters 对应的字（没敲完的音节按上下文补全），往后自然延伸到一个意思完整的断句处，一般不超过 20 个字。\
用户常常是想不起整句怎么说才只敲了开头（suoyiwoxiangq → 所以我想去吃饭），或者敲的是简拼（fhyq → 符合要求）。\
它整个替换这段拼音：要接得上 before、连得上 after，但**不要把 before 或 after 里已有的字写进来**；没有上下文就给最常见、最自然的说法。\
一般用简体中文，上下文明显是外语时跟随上下文的语言。

示例：
输入 {"letters":"zhangtao","pinyin":"zhang'tao","syllables":2,"before":"在财务系统里新建一个","after":"","local_sentence":"张涛","local_candidates":["张涛","张","章"],"max_items":4,"want_sentence":true}
输出 {"words":[{"text":"账套","pinyin":"zhang tao"}],"sentence":"账套并设置会计期间"}
输入 {"letters":"suoyiwoxiangq","pinyin":"suo'yi'wo'xiang'q","syllables":5,"before":"中午没什么事，","after":"","local_sentence":"所以我想去","local_candidates":["所以我想去","所以","锁"],"max_items":4,"want_sentence":true}
输出 {"words":[],"sentence":"所以我想去吃饭"}
输入 {"letters":"fhyq","pinyin":"f'h'y'q","syllables":4,"before":"这份方案完全","after":"","local_sentence":"符合要求","local_candidates":["符合要求","符合","发货"],"max_items":0,"want_sentence":true}
输出 {"words":[],"sentence":"符合要求，可以直接提交"}

只输出 JSON，不解释。"#;

/// 问字模式的系统提示：用户用拼音问一个字（或一个短答案）。
pub const QUESTION_SYSTEM_PROMPT: &str = "\
你是一个拼音输入法的问字助手。用户以 ? 开头用**拼音**敲了一个问题，你会收到 JSON：\
letters（实际敲的字母，可能有错字、漏字、多字）、pinyin（输入法的切分，' 分隔，可能切错）、\
question（输入法本地把拼音转成的汉字，可能有错字，只是帮你理解问题；为空就自己还原）、max_items。

用户是**打不出某个字**才来问的：问题通常是问某个汉字——描述字形（san ge mu shi shen me zi → 森）、报部件（mu mu mu → 森）、\
描述读音或意思（biao shi gao xing de zi → 悦 / 欣 / 喜）；也可能是要一个很短的事实答案（fa guo shou du → 巴黎）。\
先把拼音还原成问题，再作答。**只给答案，绝不要把问题本身或它的汉字写法当作答案**：\
「三个直是什么字」答 矗，不答「三个直是什么字」；答案通常是一个字，几个可能的字各占一条。

输出 JSON：{\"answers\": [{\"text\": \"…\", \"pinyin\": \"…\"}]}

answers：1 到 max_items 个，按可能性排序。text 是能直接上屏的字、词或短答案，不要解释；\
pinyin 是 text 的带声调拼音（如 sēn），非中文答案给空字符串。不确定就少给，实在不懂就给空数组。";

/// 翻译的系统提示：中文选区译成学习语言，外文选区译回中文，只要译文。方向由 Core 按文字判断，模型兜底。
pub const TRANSLATE_SYSTEM_PROMPT: &str = "\
你是一个输入法的翻译助手。用户在应用里选中了一段文字并按了翻译快捷键，你会收到 JSON：\
text（选中的原文）、target_language（目标语言代码：zh 中文、en 英语、ja 日语）。\
把 text 完整、自然地译成目标语言，保留原文的语气、换行与标点习惯；\
原文已经是目标语言时：目标不是中文就改译成中文，目标是中文就原样返回。\
不要解释、不要加引号、不要加「译文：」之类的前缀。\
输出 JSON：{\"sentence\": \"译文\"}";

pub fn system_prompt(request: &PredictionRequest) -> &'static str {
    match request.kind {
        PredictionKind::Compose => SYSTEM_PROMPT,
        PredictionKind::Question => QUESTION_SYSTEM_PROMPT,
        PredictionKind::Translate => TRANSLATE_SYSTEM_PROMPT,
    }
}

/// 发给模型的用户消息：把请求原样序列化，模型看到的和我们记日志的完全一致。
#[derive(Serialize)]
struct UserMessage<'a> {
    letters: &'a str,

    pinyin: &'a str,

    syllables: usize,

    before: &'a str,

    after: &'a str,

    local_sentence: &'a str,

    local_candidates: &'a [String],

    max_items: usize,

    want_sentence: bool,
}

/// 问字模式发给模型的用户消息。
#[derive(Serialize)]
struct QuestionMessage<'a> {
    letters: &'a str,

    pinyin: &'a str,

    question: &'a str,

    max_items: usize,
}

/// 翻译发给模型的用户消息。
#[derive(Serialize)]
struct TranslateMessage<'a> {
    text: &'a str,

    target_language: &'a str,
}

pub fn user_prompt(request: &PredictionRequest) -> String {
    if request.kind == PredictionKind::Translate {
        return serde_json::to_string(&TranslateMessage {
            text: &request.text,
            target_language: &request.target_language,
        })
        .unwrap_or_default();
    }
    if request.kind == PredictionKind::Question {
        return serde_json::to_string(&QuestionMessage {
            letters: &request.letters,
            pinyin: &request.pinyin,
            question: &request.guess,
            max_items: request.max_items,
        })
        .unwrap_or_default();
    }
    serde_json::to_string(&UserMessage {
        letters: &request.letters,
        pinyin: &request.pinyin,
        syllables: request.syllables,
        before: &request.before,
        after: &request.after,
        local_sentence: &request.guess,
        local_candidates: &request.candidates,
        max_items: request.max_items,
        want_sentence: request.want_sentence,
    })
    .unwrap_or_default()
}

/// 解析后的回复。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reply {
    /// 云端词（组句联想）或答案（问字模式，音节为空、带读音）。
    pub words: Vec<CloudWord>,

    /// 整句补全。
    pub sentence: Option<String>,
}

impl Reply {
    /// 什么都没给：没有词也没有整句。
    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.sentence.is_none()
    }
}

/// 模型回复的原始形状，缺的字段当空。
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawReply {
    words: Vec<RawWord>,

    sentence: Option<String>,

    /// 问字模式的答案。
    answers: Vec<RawWord>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawWord {
    text: String,

    pinyin: String,
}

/// 解析模型回复：去空、去重、去换行，截到 `max_items`。不要与本地首选相同的词，也不要没给拼音的词。
pub fn parse_reply(content: &str, request: &PredictionRequest) -> Reply {
    let raw: RawReply = match serde_json::from_str(content.trim()) {
        Ok(raw) => raw,
        Err(_) => return Reply::default(),
    };
    if request.kind == PredictionKind::Question {
        return parse_answers(raw.answers, request.max_items);
    }
    if request.kind == PredictionKind::Translate {
        // 译文保留换行（原文可能是多段），只去首尾空白
        return Reply {
            words: Vec::new(),
            sentence: raw
                .sentence
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
        };
    }
    let mut reply = Reply::default();
    let mut seen: Vec<String> = Vec::new();
    let first_local = request.candidates.first().map(String::as_str);
    for word in raw.words {
        let text = clean(&word.text);
        let syllables: Vec<String> = word
            .pinyin
            .split(|c: char| c.is_whitespace() || c == '\'')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase())
            .collect();
        if text.is_empty()
            || syllables.is_empty()
            || Some(text.as_str()) == first_local
            || seen.contains(&text)
        {
            continue;
        }
        seen.push(text.clone());
        reply.words.push(CloudWord {
            text,
            syllables,
            reading: None,
        });
        if reply.words.len() >= request.max_items {
            break;
        }
    }
    if request.want_sentence {
        reply.sentence = raw
            .sentence
            .map(|s| strip_before(&clean(&s), &request.before))
            .filter(|s| !s.is_empty() && Some(s.as_str()) != first_local);
    }
    reply
}

/// 问字模式的答案：去空、去重，拼音只是显示用的读音，不校验。
fn parse_answers(answers: Vec<RawWord>, max_items: usize) -> Reply {
    let mut reply = Reply::default();
    for answer in answers {
        let text = clean(&answer.text);
        if text.is_empty() || reply.words.iter().any(|w| w.text == text) {
            continue;
        }
        let reading = clean(&answer.pinyin);
        reply.words.push(CloudWord {
            text,
            syllables: Vec::new(),
            reading: (!reading.is_empty()).then_some(reading),
        });
        if reply.words.len() >= max_items {
            break;
        }
    }
    reply
}

/// 模型爱把 before 也抄进整句里；整句只替换拼音，所以把与 before 尾部重叠的开头去掉。
fn strip_before(sentence: &str, before: &str) -> String {
    let before: Vec<char> = before.chars().collect();
    let chars: Vec<char> = sentence.chars().collect();
    // 从最长的重叠开始试：before 的后 k 个字符 == sentence 的前 k 个字符
    for k in (1..=before.len().min(chars.len())).rev() {
        if before[before.len() - k..] == chars[..k] {
            return chars[k..]
                .iter()
                .collect::<String>()
                .trim_start()
                .to_owned();
        }
    }
    sentence.to_owned()
}

fn clean(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|c| *c != '\n' && *c != '\r')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(pinyin: &str, want_sentence: bool) -> PredictionRequest {
        PredictionRequest {
            sequence: 1,
            kind: PredictionKind::Compose,
            before: "我们今天".into(),
            after: String::new(),
            pinyin: pinyin.into(),
            letters: pinyin.replace('\'', ""),
            syllables: 2,
            candidates: vec!["张涛".into(), "张贴".into()],
            guess: String::new(),
            max_items: 2,
            want_sentence,
            text: String::new(),
            target_language: String::new(),
        }
    }

    #[test]
    fn user_prompt_is_the_request_as_json() {
        let prompt = user_prompt(&request("zhang'tao", true));
        assert!(prompt.contains("\"letters\":\"zhangtao\""));
        assert!(prompt.contains("\"pinyin\":\"zhang'tao\""));
        assert!(prompt.contains("\"syllables\":2"));
        assert!(prompt.contains("\"local_candidates\":[\"张涛\",\"张贴\"]"));
    }

    #[test]
    fn reply_keeps_words_with_pinyin_and_drops_the_local_first() {
        let reply = r#"{"words": [{"text": "账套", "pinyin": "zhang tao"}, {"text": "张涛", "pinyin": "zhang tao"},
            {"text": "涨停", "pinyin": ""}, {"text": " 章台 ", "pinyin": "Zhang'Tai"}, {"text": "张套", "pinyin": "zhang tao"}],
            "sentence": " 账套已经建好了\n"}"#;
        let parsed = parse_reply(reply, &request("zhang'tao", true));
        let texts: Vec<(&str, Vec<&str>)> = parsed
            .words
            .iter()
            .map(|w| {
                (
                    w.text.as_str(),
                    w.syllables.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            texts,
            [
                ("账套", vec!["zhang", "tao"]),
                ("章台", vec!["zhang", "tai"])
            ]
        );
        assert_eq!(parsed.sentence.as_deref(), Some("账套已经建好了"));
        // 没要整句就不收
        assert_eq!(
            parse_reply(reply, &request("zhang'tao", false)).sentence,
            None
        );
        // 整句里抄了 before 的，去掉重叠部分
        let echoed = r#"{"words": [], "sentence": "我们今天账套已经建好了"}"#;
        assert_eq!(
            parse_reply(echoed, &request("zhang'tao", true))
                .sentence
                .as_deref(),
            Some("账套已经建好了")
        );
        let partial = r#"{"words": [], "sentence": "今天账套已经建好了"}"#;
        assert_eq!(
            parse_reply(partial, &request("zhang'tao", true))
                .sentence
                .as_deref(),
            Some("账套已经建好了")
        );
    }

    #[test]
    fn question_replies_keep_answers_with_readings() {
        let mut question = request("san'ge'mu", false);
        question.kind = PredictionKind::Question;
        question.candidates.clear();
        assert!(user_prompt(&question).contains("\"letters\":\"sangemu\""));
        assert!(!user_prompt(&question).contains("before"));
        let reply = r#"{"answers": [{"text": "森", "pinyin": "sēn"}, {"text": "森", "pinyin": "sēn"}, {"text": "巴黎", "pinyin": ""}]}"#;
        let parsed = parse_reply(reply, &question);
        assert_eq!(parsed.words.len(), 2);
        assert_eq!(parsed.words[0].text, "森");
        assert_eq!(parsed.words[0].reading.as_deref(), Some("sēn"));
        assert!(parsed.words[0].syllables.is_empty());
        assert_eq!(parsed.words[1].reading, None);
        assert_eq!(parsed.sentence, None);
    }

    #[test]
    fn garbage_replies_are_empty() {
        assert_eq!(
            parse_reply("not json", &request("k", false)),
            Reply::default()
        );
        assert_eq!(
            parse_reply(r#"{"foo": 1}"#, &request("zt", false)),
            Reply::default()
        );
    }

    #[test]
    fn translate_requests_use_their_own_prompt_and_keep_the_translation() {
        let mut translate = request("", false);
        translate.kind = PredictionKind::Translate;
        translate.text = "我想去吃饭".to_owned();
        translate.target_language = "en".to_owned();
        assert_eq!(system_prompt(&translate), TRANSLATE_SYSTEM_PROMPT);
        assert!(user_prompt(&translate).contains("\"target_language\":\"en\""));
        let reply = parse_reply(r#"{"sentence": "  I want to go eat.\n"}"#, &translate);
        assert_eq!(reply.sentence.as_deref(), Some("I want to go eat."));
        assert!(reply.words.is_empty());
        assert!(
            parse_reply(r#"{"sentence": ""}"#, &translate)
                .sentence
                .is_none()
        );
    }
}
