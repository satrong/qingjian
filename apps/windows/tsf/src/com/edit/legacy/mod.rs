//! 标准 Win32 Edit 的上下文读取：旧式控件的 TSF 范围可能只覆盖组句。
//! 只读当前文档、当前线程内的标准输入框，不枚举其他窗口或跨进程取文本。

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Controls::{EM_GETPASSWORDCHAR, EM_GETSEL};
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::TextServices::ITfContext;
use windows::Win32::UI::WindowsAndMessaging::{
    ES_PASSWORD, GWL_STYLE, GetClassNameW, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsChild, IsWindowUnicode, SendMessageW,
};

use crate::com::edit::surrounding::InputContext;

/// 标准 Edit 没有按任意区间取文本的消息，限制整段拷贝以免大文档阻塞输入。
const MAX_TEXT_UNITS: usize = 1024 * 1024;

pub(super) fn input_context(
    context: &ITfContext,
    before: usize,
    after: usize,
) -> Option<InputContext> {
    let view = unsafe { context.GetActiveView() }.ok()?;
    let document = unsafe { view.GetWnd() }.ok()?;
    let focused = unsafe { GetFocus() };
    if focused.is_invalid()
        || (document != focused && !unsafe { IsChild(document, focused) }.as_bool())
    {
        return None;
    }
    read_edit(focused, before, after)
}

fn read_edit(window: HWND, before: usize, after: usize) -> Option<InputContext> {
    if unsafe { GetWindowThreadProcessId(window, None) } != unsafe { GetCurrentThreadId() }
        || !unsafe { IsWindowUnicode(window) }.as_bool()
    {
        return None;
    }
    let mut class = [0u16; 32];
    let len = unsafe { GetClassNameW(window, &mut class) } as usize;
    if String::from_utf16_lossy(&class[..len]) != "Edit" {
        return None;
    }
    if unsafe { GetWindowLongW(window, GWL_STYLE) } & ES_PASSWORD != 0
        || unsafe { SendMessageW(window, EM_GETPASSWORDCHAR, None, None) }.0 != 0
    {
        return Some(InputContext {
            private: true,
            ..InputContext::default()
        });
    }
    let len = usize::try_from(unsafe { GetWindowTextLengthW(window) }).ok()?;
    if len > MAX_TEXT_UNITS {
        return None;
    }
    let (mut start, mut end) = (0u32, 0u32);
    // 不能拆 SendMessage 的返回值：其两个选区偏移只有 16 位，大文档会截断。
    unsafe {
        SendMessageW(
            window,
            EM_GETSEL,
            Some(WPARAM((&mut start as *mut u32) as usize)),
            Some(LPARAM((&mut end as *mut u32) as isize)),
        );
    }
    let mut text = vec![0u16; len + 1];
    let copied = usize::try_from(unsafe { GetWindowTextW(window, &mut text) }).ok()?;
    let (start, end) = (start as usize, end as usize);
    if start > end || end > copied {
        return None;
    }
    let prefix = String::from_utf16_lossy(&text[..start]);
    let suffix = String::from_utf16_lossy(&text[end..copied]);
    Some(InputContext {
        private: false,
        before: prefix
            .chars()
            .skip(prefix.chars().count().saturating_sub(before))
            .collect(),
        after: suffix.chars().take(after).collect(),
    })
}

#[cfg(test)]
mod tests;
