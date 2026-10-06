//! 「手机输入」面板：一个二维码、一条地址、一个状态与三个按钮。
//!
//! 二维码用系统的 CoreImage（`CIQRCodeGenerator`）出，不自己实现 QR 编码——
//! 编码表、纠错与掩码选择有一堆细节，写错时的症状是「扫不出来」，很难查；系统的实现免费且不会错。
//! 生成不出来时只显示地址，复制地址仍然可用。
//!
//! 面板是普通 NSWindow：输入法进程平时 `LSBackgroundOnly`，开面板前把激活策略切成 Accessory，
//! 关窗时切回去，和偏好设置窗口同一套做法。

use std::cell::RefCell;

use objc2::exception::catch;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBezelStyle,
    NSBitmapImageRep, NSButton, NSColor, NSCompositingOperation, NSFont, NSImage, NSTextField,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_core_image::{CIContext, CIFilter};
use objc2_foundation::{NSData, NSPoint, NSRect, NSSize, NSString};
use qingjian_platform::RemoteConfig;

use super::PanelAction;
use crate::host;

/// 面板宽高。
const SIZE: NSSize = NSSize::new(340.0, 400.0);

/// 左边留白。
const PAD: f64 = 20.0;

/// 二维码边长（点）。
const QR_SIZE: f64 = 196.0;

/// 二维码纠错等级：M（约 15%）。地址只有几十个字符，版本小且扫得稳。
const CORRECTION_LEVEL: &str = "M";

/// `QrView` 的 ivar：一张现成的二维码位图，没有时画空白。
type QrImage = RefCell<Option<Retained<NSImage>>>;

define_class!(
    // SAFETY: NSView 允许子类化；没有实现 Drop。
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = QrImage]
    /// 只画一张二维码的视图，图像按视图尺寸整块贴上。
    pub struct QrView;

    impl QrView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _rect: &NSRect) {
            let image = self.ivars().borrow();
            let Some(image) = &*image else {
                return;
            };
            let bounds = self.bounds();
            let side = bounds.size.width.min(bounds.size.height);
            let target = NSRect::new(
                NSPoint::new(
                    (bounds.size.width - side) / 2.0,
                    (bounds.size.height - side) / 2.0,
                ),
                NSSize::new(side, side),
            );
            // drawRect: 里一定有当前图形上下文
            image.drawInRect_fromRect_operation_fraction(
                target,
                NSRect::ZERO,
                NSCompositingOperation::Copy,
                1.0,
            );
        }
    }
);

unsafe impl objc2::runtime::NSObjectProtocol for QrView {}

impl QrView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(RefCell::new(None));
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this
    }

    fn set_image(&self, image: Option<Retained<NSImage>>) {
        self.ivars().replace(image);
        self.setNeedsDisplay(true);
    }
}

define_class!(
    // SAFETY: NSWindow 允许子类化；没有实现 Drop。
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    /// 配对面板的窗口：关窗时把激活策略切回 Prohibited，输入法回到纯后台。
    pub struct PanelWindow;

    impl PanelWindow {
        #[unsafe(method(close))]
        fn close(&self) {
            let mtm = MainThreadMarker::from(self);
            NSApplication::sharedApplication(mtm)
                .setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
            let _: () = unsafe { msg_send![super(self), close] };
        }
    }
);

unsafe impl objc2::runtime::NSObjectProtocol for PanelWindow {}

define_class!(
    // SAFETY: NSObject 没有子类化要求；target 由面板持有一份，不会先于按钮释放。
    #[unsafe(super(objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    /// 面板按钮的 target：三个按钮都发 `pressed:`，按 tag 认。
    pub struct PanelTarget;

    impl PanelTarget {
        #[unsafe(method(pressed:))]
        fn pressed(&self, sender: Option<&objc2::runtime::AnyObject>) {
            let Some(action) = super::panel_action_from_sender(sender) else {
                return;
            };
            match action {
                PanelAction::Toggle => {
                    host::with(|host| host.toggle_remote());
                }
                other => {
                    host::with(|host| host.remote.panel_action(other));
                }
            }
        }
    }
);

unsafe impl objc2::runtime::NSObjectProtocol for PanelTarget {}

impl PanelTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// 面板本体：窗口与几个要随时改内容的控件。关窗不销毁，下次开继续用。
pub struct RemotePanel {
    window: Retained<PanelWindow>,
    qr: Retained<QrView>,
    address: Retained<NSTextField>,
    status: Retained<NSTextField>,
    toggle: Retained<NSButton>,
    _target: Retained<PanelTarget>,
}

impl RemotePanel {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let allocated = mtm.alloc::<PanelWindow>().set_ivars(());
        let window: Retained<PanelWindow> = unsafe {
            msg_send![
                super(allocated),
                initWithContentRect: NSRect::new(NSPoint::ZERO, SIZE),
                styleMask: NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                backing: NSBackingStoreType::Buffered,
                defer: false,
            ]
        };
        // 程序建的 NSWindow 默认关窗即释放，面板还要留着复用
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("手机输入"));
        // SAFETY: 无参数的系统方法，把窗口挪到所属屏幕中央
        unsafe {
            let _: () = msg_send![&*window, center];
        }

        let target = PanelTarget::new(mtm);
        let content = window.contentView().expect("面板窗口有内容视图");
        // AppKit 原点在左下，从底部往上摆控件
        let mut y = SIZE.height - 20.0 - 42.0;
        content.addSubview(&label(
            mtm,
            "手机和电脑连同一个 Wi-Fi，用手机相机扫下面这个码，\n在浏览器里说话，文本会插进电脑上的输入框。",
            NSRect::new(NSPoint::new(PAD, y), NSSize::new(SIZE.width - 2.0 * PAD, 42.0)),
        ));

        y -= 14.0;
        let qr = QrView::new(
            mtm,
            NSRect::new(
                NSPoint::new((SIZE.width - QR_SIZE) / 2.0, y - QR_SIZE),
                NSSize::new(QR_SIZE, QR_SIZE),
            ),
        );
        content.addSubview(&qr);

        y -= QR_SIZE + 14.0;
        let address = label(
            mtm,
            "",
            NSRect::new(
                NSPoint::new(PAD, y),
                NSSize::new(SIZE.width - 2.0 * PAD, 16.0),
            ),
        );
        address.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(11.0, 0.0)));
        content.addSubview(&address);

        y -= 22.0;
        let status = label(
            mtm,
            "",
            NSRect::new(
                NSPoint::new(PAD, y),
                NSSize::new(SIZE.width - 2.0 * PAD, 16.0),
            ),
        );
        status.setFont(Some(&NSFont::systemFontOfSize_weight(11.0, 0.0)));
        content.addSubview(&status);

        y -= 16.0 + 28.0;
        let button_width = (SIZE.width - 2.0 * PAD - 20.0) / 3.0;
        let mut x = PAD;
        let mut buttons = Vec::new();
        for (title, action) in [
            ("开启", PanelAction::Toggle),
            ("复制地址", PanelAction::CopyAddress),
            ("复制令牌", PanelAction::CopyToken),
        ] {
            let button = button(
                mtm,
                title,
                action,
                &target,
                NSRect::new(NSPoint::new(x, y), NSSize::new(button_width, 28.0)),
            );
            content.addSubview(&button);
            buttons.push(button);
            x += button_width + 10.0;
        }
        let [toggle, _, _] = <[Retained<NSButton>; 3]>::try_from(buttons).expect("刚放了三个按钮");

        Self {
            window,
            qr,
            address,
            status,
            toggle,
            _target: target,
        }
    }

    /// 打开面板并按当前配置刷新内容。
    ///
    /// `base` 是不带令牌的地址（取不到局域网 IP 时是占位符），`address` 是二维码与「复制地址」用的
    /// 完整地址（带令牌，取不到 IP 时退回 `base`）。两个都由 [`super::RemoteInput`] 算好后传进来：
    /// 面板自己去问 Host 会重入借用（面板就是被 Host 调开的）。
    pub fn show(&self, config: &RemoteConfig, base: &str, address: &str, summary: &str) {
        let mtm = MainThreadMarker::from(&*self.window);
        self.refresh(config, base, address, summary);
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        self.window.makeKeyAndOrderFront(None);
    }

    /// 服务刚开或刚关时刷新面板上的地址、二维码与状态。
    pub fn refresh(&self, config: &RemoteConfig, base: &str, address: &str, summary: &str) {
        self.address.setStringValue(&NSString::from_str(address));
        self.status.setStringValue(&NSString::from_str(summary));
        self.toggle.setTitle(&NSString::from_str(if config.enabled {
            "关闭"
        } else {
            "开启"
        }));
        self.qr.set_image(qr_image(address));
        // 只记不带令牌的那条
        tracing::debug!(base, enabled = config.enabled, "面板已刷新");
    }
}

/// 一行文字（`NSTextField` 的 label，够用且不用管绘制）。
fn label(mtm: MainThreadMarker, text: &str, frame: NSRect) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    field.setFrame(frame);
    field.setFont(Some(&NSFont::systemFontOfSize_weight(12.0, 0.0)));
    field.setTextColor(Some(&NSColor::labelColor()));
    field
}

/// 一个按钮，tag 记动作。
fn button(
    mtm: MainThreadMarker,
    title: &str,
    action: PanelAction,
    target: &PanelTarget,
    frame: NSRect,
) -> Retained<NSButton> {
    // SAFETY: title 与 target 都是刚建的，选择器在 PanelTarget 上有定义
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(sel!(pressed:)),
            mtm,
        )
    };
    button.setTag(tag_of(action));
    #[allow(deprecated)] // macOS 26 起圆角按钮走 bezelStyle，新 API 与旧系统不兼容
    button.setBezelStyle(NSBezelStyle::Rounded);
    button.setFrame(frame);
    button
}

/// 按钮 tag。
fn tag_of(action: PanelAction) -> isize {
    match action {
        PanelAction::Toggle => 1,
        PanelAction::CopyAddress => 2,
        PanelAction::CopyToken => 3,
    }
}

/// 文本 → 二维码位图。CoreImage 出的是 CIImage，渲染成位图后交给 `QrView` 画。
///
/// 整段包在 [`catch`] 里：CoreImage 抛的是 ObjC 异常，Rust 接不住，会直接 abort 整个进程——
/// 一个二维码画不出来不该把输入法带走。失败返回 `None`，面板上少个码，地址那行仍然能复制。
fn qr_image(text: &str) -> Option<Retained<NSImage>> {
    catch(|| qr_bitmap(text)).unwrap_or_else(|exception| {
        tracing::warn!(?exception, "生成二维码时 CoreImage 抛了异常");
        None
    })
}

/// [`qr_image`] 里真正干活的一段。
fn qr_bitmap(text: &str) -> Option<Retained<NSImage>> {
    // SAFETY: filter 名是系统自带的二维码生成器；CoreImage 自己保证对象生命周期
    let output = unsafe {
        let filter = CIFilter::filterWithName(&NSString::from_str("CIQRCodeGenerator"))?;
        // inputMessage 要 NSData：给 NSString 的话 CoreImage 会在 outputImage 上抛异常
        let message = NSData::with_bytes(text.as_bytes());
        set_filter_value(&filter, "inputMessage", &message);
        set_filter_value(
            &filter,
            "inputCorrectionLevel",
            &NSString::from_str(CORRECTION_LEVEL),
        );
        filter.outputImage()?
    };
    // SAFETY: 无参数的构造，CoreImage 全局上下文
    let context = unsafe { CIContext::new() };
    // SAFETY: 上下文与图片都是刚建的，渲染进自己的一块缓冲区
    let cg_image = unsafe { context.createCGImage_fromRect(&output, output.extent()) }?;
    // 两个对象都是刚 alloc 的；图片由上面的上下文渲染而来
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
    // SAFETY: 同上，空图片随后挂上刚做好的表示
    let image = unsafe { NSImage::initWithSize(NSImage::alloc(), output.extent().size) };
    image.addRepresentation(&rep);
    Some(image)
}

/// CoreImage 的参数走 KVC，键与值都是对象。
fn set_filter_value(filter: &CIFilter, key: &str, value: &AnyObject) {
    // SAFETY: CIFilter 遵循 KVC，键名与类型在上面写定
    unsafe {
        let _: () = msg_send![filter, setValue: value, forKey: &*NSString::from_str(key)];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_tags_match_the_actions() {
        assert_eq!(tag_of(PanelAction::Toggle), 1);
        assert_eq!(PanelAction::from_tag(1), Some(PanelAction::Toggle));
        assert_eq!(
            PanelAction::from_tag(tag_of(PanelAction::CopyToken)),
            Some(PanelAction::CopyToken)
        );
        assert_eq!(PanelAction::from_tag(9), None);
    }

    // 二维码本身没有单测：CoreImage 与 AppKit 都要求主线程，单元测试跑在别的线程上。
    // 真机验证走 bundle.sh --install 后开面板，对着二维码用手机相机扫（扫不出来就是这里坏了）。
}
