//! 在组句写入前读取选区两端的上下文，同时检查输入框的私密属性。
//! 复用异步读写会话，不把被替换的选中文字或行内拼音包含在前后文中。

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Variant::VT_UNKNOWN;
use windows::Win32::UI::TextServices::{
    GUID_PROP_INPUTSCOPE, IS_ALPHANUMERIC_PIN, IS_NUMERIC_PASSWORD, IS_NUMERIC_PIN, IS_PASSWORD,
    IS_PRIVATE, ITfContext, ITfInputScope, ITfRange, InputScope, TF_ANCHOR_END, TF_ANCHOR_START,
};
use windows::core::Interface;

use super::anchor::selection_range;

/// 限制单侧读取的内存与应用调用开销；Core 仍按用户观察窗口裁剪发送内容。
const MAX_CONTEXT_CHARS: usize = 4096;

#[derive(Default)]
pub(crate) struct InputContext {
    /// 私密 / 密码 / PIN 输入框不读取、不发送上下文。
    pub(crate) private: bool,

    pub(crate) before: String,

    pub(crate) after: String,
}

pub(crate) fn input_context(
    context: &ITfContext,
    ec: u32,
    before: usize,
    after: usize,
) -> InputContext {
    let Some(range) = selection_range(context, ec) else {
        return InputContext::default();
    };
    if private_input(context, ec, &range) {
        crate::com::log::log("私密输入框，不读光标前后文");
        return InputContext {
            private: true,
            ..InputContext::default()
        };
    }
    InputContext {
        private: false,
        before: text_near_selection(
            &range,
            ec,
            before.max(qingjian_core::RESCORE_CONTEXT_CHARS),
            true,
        )
        .unwrap_or_default(),
        after: text_near_selection(&range, ec, after, false).unwrap_or_default(),
    }
}

fn text_near_selection(
    selection: &ITfRange,
    ec: u32,
    count: usize,
    before: bool,
) -> Option<String> {
    let count = count.min(MAX_CONTEXT_CHARS);
    if count == 0 {
        return Some(String::new());
    }
    let range = unsafe { selection.Clone() }.ok()?;
    let mut shifted = 0;
    // 每个 Unicode 字符至多占两个 UTF-16 单元，不能直接把字符数当作 Shift 的距离。
    let units = (count * 2) as i32;
    unsafe {
        range
            .Collapse(
                ec,
                if before {
                    TF_ANCHOR_START
                } else {
                    TF_ANCHOR_END
                },
            )
            .ok()?;
        if before {
            range
                .ShiftStart(ec, -units, &mut shifted, std::ptr::null())
                .ok()?;
        } else {
            range
                .ShiftEnd(ec, units, &mut shifted, std::ptr::null())
                .ok()?;
        }
    }
    let mut buf = vec![0u16; units as usize];
    let mut fetched = 0;
    unsafe { range.GetText(ec, 0, &mut buf, &mut fetched) }.ok()?;
    let text = String::from_utf16_lossy(&buf[..fetched as usize]);
    let skip = if before {
        text.chars().count().saturating_sub(count)
    } else {
        0
    };
    Some(text.chars().skip(skip).take(count).collect())
}

/// 算作私密的输入范围：密码 / PIN 之外还有 `IS_PRIVATE`——Chromium（Edge / Chrome）给密码框与无痕窗口里所有输入框报的
/// 都是它（含义是「别学」），不是 `IS_PASSWORD`。真正的密码框另有 `KEYBOARD_DISABLED` compartment 让整键放行
/// （见 [`crate::com::context`]），到不了这里；这里兜的是没禁键盘但声明了私密的输入框：照常组句，但不读前文、不学、不记、不发云端。
const SECRET_SCOPES: [InputScope; 5] = [
    IS_PASSWORD,
    IS_PRIVATE,
    IS_NUMERIC_PASSWORD,
    IS_NUMERIC_PIN,
    IS_ALPHANUMERIC_PIN,
];

/// 输入框声明了私密类输入范围（`GUID_PROP_INPUTSCOPE` 里含 [`SECRET_SCOPES`] 之一）。拿不到属性按不私密。
fn private_input(context: &ITfContext, ec: u32, range: &ITfRange) -> bool {
    match input_scopes(context, ec, range) {
        Ok(scopes) => {
            crate::com::log::log(&format!("输入范围: {scopes:?}"));
            scopes.iter().any(|scope| SECRET_SCOPES.contains(scope))
        }
        // 不支持输入范围属性的应用（如记事本）GetValue 会失败，按不私密，不记日志
        Err(_) => false,
    }
}

/// 应用给 `range` 声明的全部输入范围；哪一步拿不到就说哪一步。
fn input_scopes(
    context: &ITfContext,
    ec: u32,
    range: &ITfRange,
) -> Result<Vec<InputScope>, String> {
    let property = unsafe { context.GetAppProperty(&GUID_PROP_INPUTSCOPE) }
        .map_err(|error| format!("GetAppProperty {error}"))?;
    let value =
        unsafe { property.GetValue(ec, range) }.map_err(|error| format!("GetValue {error}"))?;
    // SAFETY: 只在 vt 是 VT_UNKNOWN 时读 punkVal 那个联合体成员。
    let scope: ITfInputScope = unsafe {
        let inner = &value.Anonymous.Anonymous;
        if inner.vt != VT_UNKNOWN {
            return Err(format!("vt={}", inner.vt.0));
        }
        inner
            .Anonymous
            .punkVal
            .as_ref()
            .ok_or_else(|| "punkVal 空".to_owned())?
            .cast()
            .map_err(|error| format!("cast ITfInputScope {error}"))?
    };
    let mut scopes: *mut InputScope = std::ptr::null_mut();
    let mut count = 0u32;
    unsafe { scope.GetInputScopes(&mut scopes, &mut count) }
        .map_err(|error| format!("GetInputScopes {error}"))?;
    if scopes.is_null() {
        return Err("GetInputScopes 返回空数组".to_owned());
    }
    // SAFETY: GetInputScopes 返回 count 个元素的 CoTaskMem 数组，由调用方释放。
    unsafe {
        let list = std::slice::from_raw_parts(scopes, count as usize).to_vec();
        CoTaskMemFree(Some(scopes.cast()));
        Ok(list)
    }
}
