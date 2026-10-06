//! 「手机输入」面板：一个二维码、一条地址、一个状态与三个按钮。
//!
//! 二维码用系统的 CoreImage（`CIQRCodeGenerator`）出，不自己实现 QR 编码——
//! 编码表、纠错与掩码选择有一堆细节，写错时的症状是「扫不出来」，很难查；系统的实现免费且不会错。
//! 生成不出来时只显示地址，复制地址仍然可用。
//!
//! **清晰度**：`CIQRCodeGenerator` 的原生输出是 1 像素 = 1 模块（这条地址是 31×31），
//! 直接把它拉进面板那么大的方块里，AppKit 会按默认插值做重采样，出来的边缘全是灰的——
//! 实测 67% 的像素是中间灰，扫十次有几次扫不出来。所以这里按整数倍放大（每模块占整数个像素，
//! 模块边界正好落在像素线上），再把边长凑成设备像素的整数倍，最后 1:1 贴图、不做任何缩放。
//! 静区也补到规范要求的 4 模块（CoreImage 只自带 1 模块），白底由 [`QrView`] 画。
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
    NSBitmapImageRep, NSButton, NSColor, NSCompositingOperation, NSFont, NSGraphicsContext,
    NSImage, NSImageInterpolation, NSScreen, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::{CGAffineTransform, CGRect};
use objc2_core_image::{CIContext, CIFilter, CIImage};
use objc2_foundation::{NSData, NSPoint, NSRect, NSSize, NSString};
use qingjian_platform::RemoteConfig;

use super::PanelAction;
use crate::host;

/// 面板宽高。高度按 [`QR_SIZE`] 与其余控件累加得出，见 `RemotePanel::new` 里的布局。
const SIZE: NSSize = NSSize::new(340.0, 470.0);

/// 左边留白。
const PAD: f64 = 20.0;

/// 二维码占位的边长（点），也就是给视图留的方框。
///
/// 位图实际边长由 [`qr_side_pixels`] 按屏幕缩放算出来，**不一定正好等于这个数**——
/// 37 个模块凑不满整数倍的点尺寸。差的那几像素由 `QrView` 居中留白，不会拉伸位图。
/// 留大方框是为了给位图留出向上取整的余量：宁可多一点白边，也不要缩放。
const QR_SIZE: f64 = 260.0;

/// 二维码纠错等级：M（约 15%）。地址只有几十个字符，版本小且扫得稳。
const CORRECTION_LEVEL: &str = "M";

/// 静区宽度（模块数），QR 规范要求至少 4。CoreImage 自带 1 模块，所以每边再补 3。
const QUIET_MODULES: usize = 4;

/// 每模块最少几个像素。定这个下限是为了兜住小屏：宁可图小一点，也不要每模块只剩一两个像素。
const MIN_MODULE_PIXELS: usize = 2;

/// 一行文字（地址、状态）的高度，点。
const ADDRESS_H: f64 = 16.0;

/// 二维码下方到地址文字的**视觉**间距，点。
///
/// 布局里不直接用它，而是走 [`qr_to_address_offset`] 换算成「该减多少」——那个 14pt 的坑就是这么来的：
/// 数字看着合理，换算后视觉间距却是 −1.5pt。地址标签是 16pt 高的矩形、文字只占上面一小条，
/// 标签矩形探进二维码那一段并不会真的碰到码，所以换算必须显式做。
const QR_TO_ADDRESS_GAP: f64 = 20.0;

/// 布局该减掉的间隔：让视觉间距正好等于 [`QR_TO_ADDRESS_GAP`]。
///
/// 三项相减——位图按自身尺寸居中于方框，单边各留 `(方框 - 位图) / 2`；地址标签顶边还要再抬一个
/// [`ADDRESS_H`]；标签矩形本身的探入量得补回来。
fn qr_to_address_offset(image_side: f64) -> f64 {
    ADDRESS_H + QR_TO_ADDRESS_GAP - (QR_SIZE - image_side) / 2.0
}

/// 当前屏幕上，二维码位图会是多少点见方。
///
/// 布局与 [`qr_bitmap`] 都用它，两边必须算出同一个数，否则间隔又对不上。
fn current_image_side() -> f64 {
    let scale = backing_scale();
    let (side_pixels, _) = qr_side_pixels(QR_SIZE, scale, typical_native_modules());
    image_side_points(side_pixels, scale)
}

/// 典型配对地址在 CoreImage 里的原生边长（模块数 + 自带 1 模块白边）。
///
/// 地址长度会变：令牌固定 32 个十六进制字符，长度只随 IP 变化（IPv4 最长 15 字符、
/// IPv6 带方括号最长 47），所以模块数落在一个窄区间里。**取小的那一头**算间隔：
/// 模块越少位图越小、方框里余得越多，间隔就越窄，按最小值留白等于取最坏情况。
fn typical_native_modules() -> usize {
    // IPv4 形式的最短配对地址：`http://1.1.1.1:23333/?k=` + 32 个令牌字符
    const SHORTEST: &str = "http://1.1.1.1:23333/?k=00000000000000000000000000000000";
    let rendered = unsafe {
        let filter = CIFilter::filterWithName(&NSString::from_str("CIQRCodeGenerator"));
        filter.and_then(|filter| {
            let message = NSData::with_bytes(SHORTEST.as_bytes());
            set_filter_value(&filter, "inputMessage", &message);
            set_filter_value(
                &filter,
                "inputCorrectionLevel",
                &NSString::from_str(CORRECTION_LEVEL),
            );
            filter.outputImage()
        })
    };
    match rendered {
        // 拿不到就退回这条地址的已知值（29 模块 + 1 白边）
        Some(image) => {
            // SAFETY: 刚拿到的 CIImage
            let width = unsafe { image.extent() }.size.width;
            if width.is_finite() && width >= 1.0 {
                width as usize
            } else {
                31
            }
        }
        None => 31,
    }
}

/// `QrView` 的 ivar：一张现成的二维码位图，没有时画空白。
type QrImage = RefCell<Option<Retained<NSImage>>>;

/// `QrView` 算出的贴图矩形：位图按**自己的**点尺寸画，在方框里居中。
///
/// 单独拆出来是为了能单测——`drawRect:` 要主线程，测试跑不到，但「有没有被拉伸」
/// 这个判断本身是纯算术。返回 `None` 表示没有图像可画。
fn draw_target(bounds: NSSize, image_side: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(
            (bounds.width - image_side) / 2.0,
            (bounds.height - image_side) / 2.0,
        ),
        NSSize::new(image_side, image_side),
    )
}

define_class!(
    // SAFETY: NSView 允许子类化；没有实现 Drop。
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = QrImage]
    /// 只画一张二维码的视图：位图已经是「白底 + 纯黑白模块」，按它自己的尺寸 1:1 居中贴上。
    pub struct QrView;

    impl QrView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _rect: &NSRect) {
            let image = self.ivars().borrow();
            let Some(image) = &*image else {
                return;
            };
            // 按位图**自己的**点尺寸画，不拉满视图。位图边长是「每模块整数像素」凑出来的，
            // 除不尽方框的边长（37 模块凑不满 260pt@2x 的 520 像素），所以两者必然差一点。
            // 一旦按方框尺寸拉伸，模块宽度就变成 10/11/22/53 这种参差混搭——那才是「糊」的真正来源。
            // 差的那几像素留成白边，反正静区本来就是白的。
            let target = draw_target(self.bounds().size, image.size().width);
            // 插值显式关掉：万一尺寸对不上，也宁可变成硬边的轻微参差，不要糊成一片灰。
            let Some(context) = NSGraphicsContext::currentContext() else {
                return;
            };
            context.setImageInterpolation(NSImageInterpolation::None);
            image.drawInRect_fromRect_operation_fraction(
                target,
                NSRect::ZERO,
                NSCompositingOperation::SourceOver,
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

        y -= QR_SIZE + qr_to_address_offset(current_image_side());
        let address = label(
            mtm,
            "",
            NSRect::new(
                NSPoint::new(PAD, y),
                NSSize::new(SIZE.width - 2.0 * PAD, ADDRESS_H),
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
                NSSize::new(SIZE.width - 2.0 * PAD, ADDRESS_H),
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

/// 算二维码该渲染成多少设备像素见方，以及每模块占几个像素。
///
/// 关键是**结果要正好填满视图**：视图恒定按 [`QR_SIZE`] 见方来画（居中），
/// 而位图是按设备像素算的。两者对不上时 `drawInRect` 就会缩放，一次 1.06 倍的缩放
/// 就足以让模块宽度变成 10/11/22/53 这种参差不齐的混搭——看着就是糊的。
///
/// 两条约束：
/// - 每模块占**整数**个像素，否则模块边界落在像素中间，边缘会插值出灰
/// - 边长（点）× 缩放比 ÷ 每模块像素数 = 模块总数，除不尽就会剩下半块
///
/// 所以从「每模块几个像素」倒推边长，而不是拿可用空间去凑。
fn qr_side_pixels(available_points: f64, scale: f64, native_modules: usize) -> (usize, usize) {
    // CoreImage 自带 1 模块白边，补到规范要求的 4 模块
    let total_modules = native_modules + 2 * (QUIET_MODULES - 1);
    let available_pixels = (available_points * scale).floor().max(1.0) as usize;
    let module_pixels = (available_pixels / total_modules).max(MIN_MODULE_PIXELS);
    (total_modules * module_pixels, module_pixels)
}

/// 文本 → 灰度位图（每像素 8 位，0 是黑、255 是白），以及边长与每模块像素数。
///
/// 拆出这一段是因为它只碰 CoreImage，而 CoreImage 不要求主线程——单测能在别的线程上
/// 逐像素检查「有没有灰边」，那正是原来那个 bug 的症状。包成 [`NSBitmapImageRep`] 留在
/// [`qr_bitmap`] 里做。
fn qr_gray(text: &str, scale: f64) -> Option<(Retained<NSBitmapImageRep>, usize, usize)> {
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
    // CoreImage 的输出是 1 像素 = 1 模块，边长就是模块数
    // SAFETY: 刚拿到的 CIImage，读它的 extent 不涉及生命周期问题
    let native_modules = unsafe { output.extent() }.size.width;
    if !native_modules.is_finite() || native_modules < 1.0 {
        return None;
    }

    let (side_pixels, module_pixels) = qr_side_pixels(QR_SIZE, scale, native_modules as usize);

    // 整数倍放大：CoreImage 的仿射变换在整数倍下逐模块复制，不做重采样，边缘保持纯黑纯白。
    // 非整数倍（比如直接把 31 拉到 370）会引入灰边，这正是原来那个糊的根因。
    // SAFETY: 刚拿到的 CIImage，变换是它自己支持的操作
    let module = module_pixels as f64;
    let scaled = unsafe {
        output.imageByApplyingTransform(CGAffineTransform {
            a: module,
            b: 0.0,
            c: 0.0,
            d: module,
            tx: 0.0,
            ty: 0.0,
        })
    };
    // 往右下挪出静区：CoreImage 自带的那 1 模块不够，补到规范要求的 4 模块。
    // SAFETY: 同上
    let offset = ((QUIET_MODULES - 1) * module_pixels) as f64;
    let placed = unsafe {
        scaled.imageByApplyingTransform(CGAffineTransform {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: offset,
            ty: offset,
        })
    };
    let extent = CGRect::new(
        NSPoint::ZERO,
        NSSize::new(side_pixels as f64, side_pixels as f64),
    );

    // 合成到一张白底上再裁到目标大小。静区于是是**不透明的纯白**而不是透明：
    // 透明的话得靠视图先填白底，而位图一旦带 alpha，贴图时的合成规则就多一层不确定性。
    // SAFETY: filter 名是系统自带的合成器；白底与二维码都是刚拿到的 CIImage
    let opaque = unsafe {
        let canvas = CIImage::whiteImage().imageByCroppingToRect(extent);
        let over = CIFilter::filterWithName(&NSString::from_str("CISourceOverCompositing"))?;
        set_filter_value(&over, "inputImage", &placed);
        set_filter_value(&over, "inputBackgroundImage", &canvas);
        over.outputImage()?.imageByCroppingToRect(extent)
    };

    // SAFETY: 无参数的构造，CoreImage 全局上下文
    let context = unsafe { CIContext::new() };
    // SAFETY: 上下文与图片都是刚建的，渲染进自己的一块缓冲区
    let cg_image = unsafe { context.createCGImage_fromRect(&opaque, extent) }?;
    // 两个对象都是刚 alloc 的；图片由上面的上下文渲染而来
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
    Some((rep, side_pixels, module_pixels))
}

/// NSImage 该声明的边长（点）。
///
/// 这一步是清晰度的关键，别当成「随便给个尺寸」：视图是按**点**来画这张图的，
/// 而位图是按**设备像素**算的。两者不一致时 AppKit 只能插值缩放，出来的就是一片灰边。
/// 所以点尺寸 = 设备像素 ÷ 缩放比，1 像素对 1 像素，正好 1:1。
///
/// 原先这里直接用 CoreImage 的 `extent.size`（31 点），而实际要画到 196 点——
/// 差 6.3 倍（Retina 上 12.6 倍），67% 的像素变成灰的，这就是二维码糊掉的根因。
fn image_side_points(side_pixels: usize, scale: f64) -> f64 {
    side_pixels as f64 / scale
}

/// [`qr_image`] 里真正干活的一段。
fn qr_bitmap(text: &str) -> Option<Retained<NSImage>> {
    // 同一次生成里只问一次缩放比：问两次万一中间改了显示设置，两个尺寸就对不上了
    let scale = backing_scale();
    let (rep, side_pixels, _) = qr_gray(text, scale)?;
    let side = image_side_points(side_pixels, scale);
    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(side, side));
    image.addRepresentation(&rep);
    Some(image)
}

/// 主屏的缩放比（Retina 为 2）。拿不到时按 1 算——图小一点，但不会糊。
fn backing_scale() -> f64 {
    let Some(mtm) = MainThreadMarker::new() else {
        return 1.0;
    };
    let scale = NSScreen::mainScreen(mtm).map_or(1.0, |screen| screen.backingScaleFactor());
    if scale.is_finite() && scale >= 1.0 {
        scale
    } else {
        1.0
    }
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
    use objc2_app_kit::NSBitmapFormat;

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

    /// 这条地址在 CoreImage 里的原生边长：29 个模块 + 它自带的 1 模块白边。
    const NATIVE: usize = 31;

    /// 一条真实的配对地址，用来做二维码渲染的样本。
    const ADDRESS: &str = "http://192.168.2.199:23333/?k=f973ec773b2448";

    #[test]
    fn every_module_lands_on_a_whole_number_of_pixels() {
        // 这是清晰度的全部要害：每模块占整数个像素，模块边界才落在像素线上
        for scale in [1.0, 2.0, 3.0] {
            let (side, module) = qr_side_pixels(QR_SIZE, scale, NATIVE);
            assert!(
                module >= MIN_MODULE_PIXELS,
                "{scale}x: 每模块只有 {module} 像素"
            );
            assert_eq!(
                side % module,
                0,
                "{scale}x: 边长 {side} 除不尽每模块 {module} 像素"
            );
        }
    }

    #[test]
    fn the_quiet_zone_is_four_modules_wide() {
        // CoreImage 自带的 1 模块白边也算在静区里，所以数据区只有 29 个模块
        let data_modules = NATIVE - 2;
        for scale in [1.0, 2.0, 3.0] {
            let (side, module) = qr_side_pixels(QR_SIZE, scale, NATIVE);
            let quiet = (side - data_modules * module) / 2;
            assert_eq!(quiet, QUIET_MODULES * module, "{scale}x: 静区宽度不对");
        }
    }

    #[test]
    fn the_code_fits_in_the_space_the_panel_leaves_for_it() {
        for scale in [1.0, 2.0, 3.0] {
            let (side, _) = qr_side_pixels(QR_SIZE, scale, NATIVE);
            assert!(
                side as f64 <= QR_SIZE * scale,
                "{scale}x: 边长 {side} 超出留出的 {:.0} 设备像素",
                QR_SIZE * scale
            );
        }
    }

    #[test]
    fn a_tiny_box_still_gets_the_minimum_module_size() {
        // 地方再小也不能让每模块只剩一个像素，那比糊更糟
        let (_, module) = qr_side_pixels(1.0, 1.0, NATIVE);
        assert_eq!(module, MIN_MODULE_PIXELS);
    }

    #[test]
    fn the_image_maps_one_to_one_onto_the_pixels() {
        // 这条是「不糊」的充分条件。视图按点画图，所以「位图像素数 == 点尺寸 × 缩放比」
        // 时 AppKit 才不用插值。原来的 bug 正是这条不成立：位图 31 像素却声明成 31 点，
        // 画进 196 点的视图里就是 6.3 倍（Retina 12.6 倍）拉伸，67% 的像素变灰。
        for scale in [1.0, 2.0, 3.0] {
            let (side_pixels, _) = qr_side_pixels(QR_SIZE, scale, NATIVE);
            let points = image_side_points(side_pixels, scale);
            assert_eq!(
                points * scale,
                side_pixels as f64,
                "{scale}x: {points} 点在 {scale}x 屏上占 {} 像素，与位图的 {side_pixels} 不符",
                points * scale
            );
        }
    }

    #[test]
    fn the_image_fits_the_box_without_needing_a_rescale() {
        // QrView 按位图自己的点尺寸画、居中放进 QR_SIZE 的方框，所以位图只要**不超过**方框即可。
        // 一旦超过，视图就得缩放，模块宽度立刻变成 10/11/22/53 这种参差混搭——那才是「糊」。
        // （上一版就是这里没兜住：位图凑到 370 像素 = 185 点，视图却按 196 点拉伸，差 1.059 倍。）
        for scale in [1.0, 2.0, 3.0] {
            let (side_pixels, _) = qr_side_pixels(QR_SIZE, scale, NATIVE);
            let points = image_side_points(side_pixels, scale);
            assert!(
                points <= QR_SIZE,
                "{scale}x: 图声明 {points} 点，比留出的 {QR_SIZE} 点还大，视图就得缩放"
            );
        }
    }

    #[test]
    fn the_box_is_wide_enough_for_the_image_at_every_scale() {
        // 反过来也成立：方框不能小太多，否则位图被缩到比框还小，看着就小一号、扫起来更费劲。
        // 允许差几像素（凑整除不尽的余数），但不能差出一大截。
        for scale in [1.0, 2.0, 3.0] {
            let (side_pixels, _) = qr_side_pixels(QR_SIZE, scale, NATIVE);
            let points = image_side_points(side_pixels, scale);
            let total_modules = NATIVE + 2 * (QUIET_MODULES - 1);
            let one_module = QR_SIZE / total_modules as f64;
            assert!(
                points >= QR_SIZE - one_module,
                "{scale}x: 图只有 {points} 点，比方框 {QR_SIZE} 点小了一个模块以上（{one_module:.1}）"
            );
        }
    }

    #[test]
    fn the_qr_is_big_enough_to_scan() {
        // 别为了塞进面板把它缩得太小：一个模块在 1x 屏上至少 4 像素，Retina 上 8 像素，
        // 手机隔着桌面扫才稳。这条挡住「以后有人为了省地方把 QR_SIZE 调小」。
        for scale in [1.0, 2.0, 3.0] {
            let (_, module) = qr_side_pixels(QR_SIZE, scale, NATIVE);
            assert!(
                module >= 4,
                "{scale}x: 每模块只有 {module} 像素，手机扫会吃力"
            );
        }
    }

    /// 从布局常量反推：位图下沿到地址文字上沿之间的视觉间距，点。
    ///
    /// 布局里减的是 [`qr_to_address_offset`]，减完地址标签的**底边**落在
    /// `y_qr - offset`，而文字在标签顶边（`+ ADDRESS_H`），位图底边在 `y_qr + slack`。
    /// 所以视觉间距 = `slack + offset - ADDRESS_H`。
    fn visual_gap_below_qr(image_side: f64) -> f64 {
        let slack = (QR_SIZE - image_side) / 2.0;
        slack + qr_to_address_offset(image_side) - ADDRESS_H
    }

    #[test]
    fn the_address_does_not_touch_the_qr() {
        // 这条盯的是上一版的问题：间隔当时是 14pt，换算下来视觉间距 −1.5pt，
        // 地址那一行几乎压在码的下沿上。视觉间距得真的留出十几点才像「隔开了一行」。
        for scale in [1.0, 2.0, 3.0] {
            for native in [NATIVE, 45] {
                let (side_pixels, _) = qr_side_pixels(QR_SIZE, scale, native);
                let gap = visual_gap_below_qr(image_side_points(side_pixels, scale));
                assert!(
                    gap >= 16.0,
                    "{scale}x/{native}模块: 视觉间距只有 {gap:.1}pt，地址贴着码了"
                );
            }
        }
    }

    #[test]
    fn the_address_line_still_fits_under_the_qr() {
        // 反过来也别太松：留 40pt 以上就有点空了，面板也白占地方
        for scale in [1.0, 2.0, 3.0] {
            for native in [NATIVE, 45] {
                let (side_pixels, _) = qr_side_pixels(QR_SIZE, scale, native);
                let gap = visual_gap_below_qr(image_side_points(side_pixels, scale));
                assert!(
                    gap <= 40.0,
                    "{scale}x/{native}模块: 视觉间距 {gap:.1}pt 太松了，码和地址像是两件事"
                );
            }
        }
    }

    #[test]
    fn the_layout_offset_leaves_the_intended_gap() {
        // `qr_to_address_offset` 的定义就是「让视觉间距等于 QR_TO_ADDRESS_GAP」，直接验一遍
        for scale in [1.0, 2.0, 3.0] {
            for native in [NATIVE, 45] {
                let (side_pixels, _) = qr_side_pixels(QR_SIZE, scale, native);
                let image_side = image_side_points(side_pixels, scale);
                assert!(
                    (visual_gap_below_qr(image_side) - QR_TO_ADDRESS_GAP).abs() < 0.01,
                    "{scale}x/{native}模块: 换算没对上，视觉间距与目标差 {}",
                    (visual_gap_below_qr(image_side) - QR_TO_ADDRESS_GAP).abs()
                );
            }
        }
    }

    #[test]
    fn the_panel_is_tall_enough_for_its_contents() {
        // 面板高度是手写的累加值。改了 QR_SIZE 或各种间距后，别让最下面的按钮掉出窗口。
        // 布局从 y = SIZE.height 起，逐项往下减，减到最底（按钮）不该为负。
        // 用最坏情况：地址最短 → 位图最小 → 间隔减得最多。
        let image_side = image_side_points(qr_side_pixels(QR_SIZE, 2.0, NATIVE).0, 2.0);
        let mut y = SIZE.height;
        y -= 20.0 + 42.0; // 上边距 + 说明
        y -= 14.0; // 说明与码之间
        y -= QR_SIZE; // 二维码
        y -= qr_to_address_offset(image_side); // 码与地址之间
        y -= ADDRESS_H; // 地址
        y -= 22.0; // 地址与状态之间
        y -= ADDRESS_H; // 状态
        y -= 16.0; // 状态与按钮之间
        assert!(
            y >= 28.0,
            "按钮底边只剩 {y:.0}pt，低于自身高度 28pt，会掉出窗口"
        );
    }

    // 下面这个是最要紧的一条：它真的走了一遍绘图路径，把位图按 `draw_target` 算出的矩形
    // 画进一块与屏幕等分辨率的缓冲区，然后量每一段同色游程的宽度。
    // 只要模块宽度全是每模块像素数的整数倍，边缘就是硬边；出现 11、53 这种就说明被缩放了。
    // 上一版的 bug（位图凑到 370 像素 = 185 点，却按 196 点的方框拉伸）在这里会量到
    // 宽度集合 [10, 11, 21, 22, 42, 53, 64]。

    /// 把位图按 `target_points` 见方（`scale` 倍分辨率）画进一块灰度缓冲区，返回每一行的游程宽度。
    fn run_widths_after_drawing(
        rep: &NSBitmapImageRep,
        target_points: f64,
        scale: f64,
    ) -> Vec<usize> {
        use objc2_core_graphics::{
            CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo,
            CGInterpolationQuality,
        };

        let side = (target_points * scale).round() as usize;
        let mut buffer = vec![255u8; side * side];
        let color_space = CGColorSpace::new_device_gray().expect("设备灰度空间");
        // SAFETY: buffer 至少 side*side 字节、每行正好 side 字节，符合 8 位无 alpha 灰度的排布
        let context = unsafe {
            CGBitmapContextCreate(
                buffer.as_mut_ptr().cast(),
                side,
                side,
                8,
                side,
                Some(&color_space),
                CGImageAlphaInfo::None.0,
            )
        }
        .expect("建位图上下文");
        let image = rep.CGImage().expect("位图里有 CGImage");
        // 与 QrView 一致：关掉插值。这里要看的正是几何缩放本身造成的参差，不是插值造成的灰边
        {
            CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::None);
            // 位图上下文原点在左下、y 向上，而 CoreImage 出的图原点在左上。翻转一下，
            // 缓冲区第 y 行才对应图像自上而下的第 y 行，量游程才有意义。
            CGContext::translate_ctm(Some(&context), 0.0, side as f64);
            CGContext::scale_ctm(Some(&context), 1.0, -1.0);
            CGContext::draw_image(
                Some(&context),
                CGRect::new(NSPoint::ZERO, NSSize::new(target_points, target_points)),
                Some(&image),
            );
        }

        // 取正中一行，量游程
        let y = side / 2;
        let row: Vec<u8> = buffer[y * side..(y + 1) * side].to_vec();
        let mut widths = Vec::new();
        let mut current = row[0];
        let mut run = 1;
        for &value in &row[1..] {
            if value == current {
                run += 1;
            } else {
                widths.push(run);
                current = value;
                run = 1;
            }
        }
        widths.push(run);
        widths
    }

    #[test]
    fn drawing_the_code_never_stretches_a_module() {
        for scale in [1.0, 2.0, 3.0] {
            let (rep, side_pixels, module) = qr_gray(ADDRESS, scale).expect("CoreImage 没出图");
            let image_side = image_side_points(side_pixels, scale);
            // 方框就是 QrView 的 bounds：二维码视图留了 QR_SIZE 见方
            let target = draw_target(NSSize::new(QR_SIZE, QR_SIZE), image_side);
            let widths = run_widths_after_drawing(&rep, target.size.width, scale);

            let odd: Vec<usize> = widths.iter().copied().filter(|w| w % module != 0).collect();
            assert!(
                odd.is_empty(),
                "{scale}x: 有 {} 段的宽度不是每模块 {module} 像素的整数倍（如 {:?}）——图被缩放了",
                odd.len(),
                odd.iter().take(5).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn the_stretched_variant_really_does_produce_ragged_modules() {
        // 反向确认上面那条测试不是空转：把「按方框拉伸」这个做法跑一遍，
        // 确认它量到的宽度确实参差不齐。将来若有人改回那样写，这条会挡住。
        let scale = 2.0;
        let (rep, _, module) = qr_gray(ADDRESS, scale).expect("CoreImage 没出图");
        let stretched = run_widths_after_drawing(&rep, QR_SIZE, scale);
        let odd: Vec<usize> = stretched
            .iter()
            .copied()
            .filter(|w| w % module != 0)
            .collect();
        assert!(
            !odd.is_empty(),
            "按方框拉伸居然没有产生参差宽度，那 draw_target 就没有必要按图像尺寸算了"
        );
    }

    // 下面两个直接看 CoreImage 出来的像素。CoreImage 不要求主线程，所以单测能跑。
    // 原来那个 bug 就死在这里：把 31×31 的原生输出硬拉到面板大小，67% 的像素是灰的。

    /// 取 (x, y) 处的灰度（0 是黑、255 是白）。
    ///
    /// 位图是不透明的，通道顺序按 CoreImage 给的 `AlphaFirst` 处理——写死 RGBA 下标会把
    /// alpha 当亮度读，静区的白就会被误判成「有内容」。
    fn gray_at(rep: &NSBitmapImageRep, x: isize, y: isize) -> Option<u8> {
        if x < 0 || y < 0 || x >= rep.pixelsWide() || y >= rep.pixelsHigh() {
            return None;
        }
        let mut pixel = 0usize;
        // SAFETY: pixel 在栈上、地址有效，坐标在上面判过范围
        unsafe {
            rep.getPixel_atX_y(std::ptr::NonNull::from(&mut pixel), x, y);
        }
        let bytes = pixel.to_le_bytes();
        if rep.samplesPerPixel() < 4 {
            return Some(bytes[0]);
        }
        Some(if rep.bitmapFormat().contains(NSBitmapFormat::AlphaFirst) {
            bytes[1]
        } else {
            bytes[0]
        })
    }

    #[test]
    fn the_rendered_code_has_no_gray_pixels() {
        // 扫不出来的直接原因：模块边缘有灰。整数倍放大后每个像素必须是纯黑或纯白。
        for scale in [1.0, 2.0, 3.0] {
            let Some((rep, side, _)) = qr_gray(ADDRESS, scale) else {
                panic!("{scale}x: CoreImage 没出图");
            };
            let side = side as isize;
            assert_eq!(rep.pixelsWide(), side, "{scale}x: 位图宽度与算出的边长不符");
            let mut gray = 0;
            for y in 0..side {
                for x in 0..side {
                    if let Some(v) = gray_at(&rep, x, y)
                        && v > 8
                        && v < 247
                    {
                        gray += 1;
                    }
                }
            }
            assert_eq!(gray, 0, "{scale}x: 有 {gray} 个灰像素，边缘是糊的");
        }
    }

    #[test]
    fn every_module_is_a_solid_block() {
        // 逐模块走一遍：模块内部不能混色（混了就是被重采样过）
        let (rep, side, module) = qr_gray(ADDRESS, 2.0).expect("CoreImage 没出图");
        let (side, module) = (side as isize, module as isize);
        let data_modules = (NATIVE - 2) as isize; // CoreImage 自带 1 模块白边
        let quiet = (side - data_modules * module) / 2;
        for m in 0..data_modules {
            for py in 0..module {
                let y = quiet + m * module + py;
                let x0 = quiet + m * module;
                let first = gray_at(&rep, x0, y);
                for px in 0..module {
                    let v = gray_at(&rep, x0 + px, y);
                    assert_eq!(v, first, "模块 ({m},{py}) 内部不纯");
                }
            }
        }
    }

    #[test]
    fn the_quiet_zone_is_blank_white() {
        // 静区必须一个模块都不少，CoreImage 自带的那 1 模块也在内。四条边各扫一列/一行。
        let (rep, side, module) = qr_gray(ADDRESS, 2.0).expect("CoreImage 没出图");
        let (side, module) = (side as isize, module as isize);
        let data_modules = (NATIVE - 2) as isize;
        let quiet = (side - data_modules * module) / 2;
        let mid = side / 2;
        for x in 0..quiet {
            assert_eq!(gray_at(&rep, x, mid), Some(255), "左边第 {x} 像素不是白的");
        }
        for x in (side - quiet)..side {
            assert_eq!(gray_at(&rep, x, mid), Some(255), "右边第 {x} 像素不是白的");
        }
        for y in 0..quiet {
            assert_eq!(gray_at(&rep, mid, y), Some(255), "顶边第 {y} 像素不是白的");
        }
        for y in (side - quiet)..side {
            assert_eq!(gray_at(&rep, mid, y), Some(255), "底边第 {y} 像素不是白的");
        }
    }

    // NSImage 与 QrView 的绘制仍需主线程，单测跑不到：真机验证走 bundle.sh --install
    // 后开面板，对着二维码用手机相机扫（扫不出来就是这里坏了）。
}
