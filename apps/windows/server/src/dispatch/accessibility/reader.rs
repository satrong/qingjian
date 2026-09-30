//! 只读取前台钉钉当前聚焦的聊天框，按选区边界截取并复核焦点、选区。

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationTextRange,
    TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, TextUnit_Character,
    UIA_TextPatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::{PWSTR, Result};

use qingjian_core::SurroundingText;

pub(super) fn foreground(window: usize) -> bool {
    unsafe { GetForegroundWindow().0 as usize == window }
}

pub(super) fn target() -> Option<(usize, u32)> {
    unsafe {
        let window = GetForegroundWindow();
        if window.is_invalid() {
            return None;
        }
        let mut process = 0;
        GetWindowThreadProcessId(window, Some(&mut process));
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process).ok()?;
        let mut path = [0u16; 32768];
        let mut length = path.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        let path = String::from_utf16_lossy(&path[..length as usize]);
        if !path
            .rsplit('\\')
            .next()?
            .eq_ignore_ascii_case("DingTalk.exe")
        {
            return None;
        }
        Some((window.0 as usize, process))
    }
}

pub(super) fn read(
    window: usize,
    process: u32,
    before: usize,
    after: usize,
) -> Option<SurroundingText> {
    read_checked(window, process, before, after).ok().flatten()
}

fn read_checked(
    window: usize,
    process: u32,
    before: usize,
    after: usize,
) -> Result<Option<SurroundingText>> {
    if !foreground(window) {
        return Ok(None);
    }
    unsafe {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let element = automation.GetFocusedElement()?;
        if element.CurrentProcessId()? as u32 != process
            || element.CurrentIsPassword()?.as_bool()
            || !element.CurrentHasKeyboardFocus()?.as_bool()
            || element.CurrentClassName()? != "im_chat::InputRichTextEdit"
        {
            return Ok(None);
        }
        // 钉钉的 HWND 是 Qt 顶层窗，多个内部输入框共用；只认已验证的聊天控件。
        let root = automation.ElementFromHandle(HWND(window as *mut _))?;
        if root.CurrentProcessId()? as u32 != process {
            return Ok(None);
        }
        let pattern: IUIAutomationTextPattern = element.GetCurrentPatternAs(UIA_TextPatternId)?;
        let selection = pattern.GetSelection()?;
        if selection.Length()? != 1 {
            return Ok(None);
        }
        let range = selection.GetElement(0)?;
        let input = SurroundingText {
            before: nearby(&range, before, true)?,
            after: nearby(&range, after, false)?,
        };
        let current = automation.GetFocusedElement()?;
        let selection = pattern.GetSelection()?;
        if !foreground(window)
            || !automation.CompareElements(&element, &current)?.as_bool()
            || element.CurrentIsPassword()?.as_bool()
            || selection.Length()? != 1
            || !range.Compare(&selection.GetElement(0)?)?.as_bool()
        {
            return Ok(None);
        }
        Ok(Some(input))
    }
}

fn nearby(selection: &IUIAutomationTextRange, count: usize, before: bool) -> Result<String> {
    let count = count.min(4096);
    if count == 0 {
        return Ok(String::new());
    }
    unsafe {
        let range = selection.Clone()?;
        let start = TextPatternRangeEndpoint_Start;
        let end = TextPatternRangeEndpoint_End;
        if before {
            range.MoveEndpointByRange(end, selection, start)?;
            range.MoveEndpointByUnit(start, TextUnit_Character, -(count as i32))?;
        } else {
            range.MoveEndpointByRange(start, selection, end)?;
            range.MoveEndpointByUnit(end, TextUnit_Character, count as i32)?;
        }
        let text = range.GetText((count * 2) as i32)?.to_string();
        let skip = if before {
            text.chars().count().saturating_sub(count)
        } else {
            0
        };
        Ok(text.chars().skip(skip).take(count).collect())
    }
}
