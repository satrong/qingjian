//! 用真实隐藏的标准 Edit 控件验证选区边界、UTF-16 和密码保护。

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{EM_SETPASSWORDCHAR, EM_SETSEL};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, ES_MULTILINE, ES_PASSWORD, SendMessageW, SetWindowTextW,
    WINDOW_EX_STYLE, WINDOW_STYLE, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::read_edit;

struct Edit(HWND);

impl Edit {
    fn new(text: &str, style: i32) -> Self {
        let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let window = Self(unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("Edit"),
                w!(""),
                WS_POPUP | WINDOW_STYLE(style as u32),
                0,
                0,
                100,
                30,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        });
        unsafe {
            SetWindowTextW(window.0, PCWSTR(text.as_ptr())).unwrap();
        }
        window
    }

    fn select(&self, start: usize, end: usize) {
        unsafe {
            SendMessageW(
                self.0,
                EM_SETSEL,
                Some(WPARAM(start)),
                Some(LPARAM(end as isize)),
            );
        }
    }
}

impl Drop for Edit {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0).unwrap();
        }
    }
}

#[test]
fn reads_text_on_both_sides_of_caret() {
    let edit = Edit::new("今天一起去公园", ES_MULTILINE);
    edit.select(2, 2);
    let input = read_edit(edit.0, 64, 32).unwrap();
    assert_eq!(input.before, "今天");
    assert_eq!(input.after, "一起去公园");
}

#[test]
fn excludes_selection_and_counts_unicode_characters() {
    let edit = Edit::new("甲😀待替换𠮷乙", ES_MULTILINE);
    edit.select(3, 6);
    let input = read_edit(edit.0, 1, 1).unwrap();
    assert_eq!(input.before, "😀");
    assert_eq!(input.after, "𠮷");
}

#[test]
fn selection_offsets_are_not_truncated_to_sixteen_bits() {
    let text = format!("{}今天一起", "a".repeat(70_000));
    let edit = Edit::new(&text, ES_MULTILINE);
    edit.select(70_002, 70_002);
    let input = read_edit(edit.0, 2, 2).unwrap();
    assert_eq!(input.before, "今天");
    assert_eq!(input.after, "一起");
}

#[test]
fn password_controls_never_return_text() {
    for style in [ES_PASSWORD, 0] {
        let edit = Edit::new("secret", style);
        if style == 0 {
            unsafe {
                SendMessageW(edit.0, EM_SETPASSWORDCHAR, Some(WPARAM('*' as usize)), None);
            }
        }
        edit.select(3, 3);
        let input = read_edit(edit.0, 64, 32).unwrap();
        assert!(input.private);
        assert!(input.before.is_empty() && input.after.is_empty());
    }
}
