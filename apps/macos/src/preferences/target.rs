use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{NSTextDelegate, NSTextView, NSTextViewDelegate};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol};

use crate::host;

/// 已登记的多行文本框与它的设置项：`NSTextView` 不是 `NSControl`、`NSView.tag` 只读，
/// 哪个框写哪个键记在这里，失焦（`textDidEndEditing:`）时按对象认回来。
type TextViews = RefCell<Vec<(super::Setting, Retained<NSTextView>)>>;

define_class!(
    // SAFETY: NSObject 没有子类化要求；文本框由页面与窗口另持一份，登记表不会让它活过窗口。
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TextViews]
    /// 设置窗口所有控件的 target：单行控件都发 `changed:` 靠 tag 区分，
    /// 多行文本框失焦发 `textDidEndEditing:` 靠登记表区分。
    pub struct PreferencesTarget;

    unsafe impl NSObjectProtocol for PreferencesTarget {}
    unsafe impl NSTextDelegate for PreferencesTarget {
        // 多行文本框（「联想提示词」）失焦或关窗收尾：整段文本写回它登记的设置项。
        #[unsafe(method(textDidEndEditing:))]
        fn text_did_end_editing(&self, notification: &NSNotification) {
            let Some(object) = notification.object() else {
                return;
            };
            let Some(text) = object.downcast_ref::<NSTextView>() else {
                return;
            };
            let registered = self
                .ivars()
                .borrow()
                .iter()
                .find(|(_, view)| std::ptr::eq(&**view, text))
                .map(|(setting, _)| *setting);
            let Some(setting) = registered else {
                return;
            };
            let content = text.string().to_string();
            host::with(|h| h.change_setting(setting, super::SettingValue::Text(content)));
        }
    }
    unsafe impl NSTextViewDelegate for PreferencesTarget {}

    impl PreferencesTarget {
        #[unsafe(method(editPhrase:))]
        fn edit_phrase(&self, _sender: Option<&AnyObject>) {
            host::with(|h| h.change_setting(super::Setting::EditPhrase, super::SettingValue::Bool(false)));
        }

        #[unsafe(method(changed:))]
        fn changed(&self, sender: Option<&AnyObject>) {
            if let Some((setting, value)) = super::setting_from_sender(sender) {
                host::with(|h| h.change_setting(setting, value));
            }
        }
    }
);

impl PreferencesTarget {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(TextViews::default());
        unsafe { msg_send![super(this), init] }
    }

    /// 登记一个多行文本框：它不带 tag，失焦时按这里记的设置项写配置。
    pub fn register_text_view(&self, setting: super::Setting, view: &NSTextView) {
        self.ivars().borrow_mut().push((setting, view.retain()));
    }
}
