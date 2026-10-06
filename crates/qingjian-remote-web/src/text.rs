//! 上屏前的文本清洗：手机发来的是「人打出来的字」，但仍要去掉不该插进输入框的东西。
//!
//! 去掉的是控制字符（含 ANSI 转义里用到的 ESC）、DEL、C1 区，以及会让人看不见的双向文本控制符；
//! 保留零宽连接符（emoji 序列要用）与换行、回车制表。

/// 清洗提交文本。返回空串或出错时页面会拿到 `400`。
///
/// 换行统一成 `\n`，首尾空白去掉；清洗后为空算无效。
pub fn sanitize(raw: &str) -> Result<String, &'static str> {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            // CRLF 与 CR 都收敛成一个换行，后面接的 LF 不再重复输出。
            '\r' | '\n' => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            '\t' => out.push('\t'),
            '\u{200e}' | '\u{200f}' => {}
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {}
            _ if is_dropped_control(ch) => {}
            _ => out.push(ch),
        }
    }
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return Err("nothing to insert after cleaning");
    }
    Ok(trimmed.to_string())
}

/// C0（除 `\n` `\t` `\r`，后两者已在调用处处理）、DEL 与 C1 区。
fn is_dropped_control(ch: char) -> bool {
    matches!(ch, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_ordinary_text() {
        assert_eq!(sanitize("你好，世界。").unwrap(), "你好，世界。");
        assert_eq!(
            sanitize("mixed 中文 and ascii").unwrap(),
            "mixed 中文 and ascii"
        );
    }

    #[test]
    fn collapses_crlf_into_one_newline() {
        assert_eq!(sanitize("一\r\n二").unwrap(), "一\n二");
        assert_eq!(sanitize("一\r二").unwrap(), "一\n二");
        assert_eq!(sanitize("一\n二").unwrap(), "一\n二");
    }

    #[test]
    fn drops_control_characters_and_escape_sequences() {
        assert_eq!(sanitize("a\u{0}b\u{7}c\u{85}d").unwrap(), "abcd");
        assert_eq!(sanitize("\u{1b}[31m红\u{1b}[0m").unwrap(), "[31m红[0m");
    }

    #[test]
    fn drops_bidi_overrides_but_keeps_zero_width_joiner() {
        assert_eq!(sanitize("a\u{202e}b").unwrap(), "ab");
        assert_eq!(sanitize("a\u{2066}b").unwrap(), "ab");
        assert_eq!(
            sanitize("👨\u{200d}👩\u{200d}👧").unwrap(),
            "👨\u{200d}👩\u{200d}👧"
        );
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(sanitize("  你好 \n").unwrap(), "你好");
    }

    #[test]
    fn rejects_blank_input() {
        assert!(sanitize("   ").is_err());
        assert!(sanitize("").is_err());
        assert!(sanitize("\n\t ").is_err());
    }

    #[test]
    fn counts_bytes_for_the_limit() {
        // 上限判定按字节，中文三字节；清洗不改变这一点，交给调用方比。
        assert_eq!(sanitize("一二三").unwrap().len(), 9);
    }
}
