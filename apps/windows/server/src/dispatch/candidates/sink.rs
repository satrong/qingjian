//! 候选窗口的输出端。

use qingjian_platform::CandidateRenderer;
use qingjian_platform::protocol::{Frame, ScreenRect};
use qingjian_render::SkinThemes;

/// 候选窗口 / 状态条的画法。
///
/// 皮肤带解析好的深浅两套（不是名字）：UI 线程不读盘，
/// 皮肤文件改了签名变、这里的内容跟着变，靠 `PartialEq` 差异决定要不要下发。
#[derive(Debug, Clone, PartialEq)]
pub struct RenderSettings {
    /// 由谁画（`[general] renderer`）。
    pub renderer: CandidateRenderer,

    /// 字族名（`[general] font`），空为系统字体。
    pub font: String,

    /// 解析好的皮肤；`SkinThemes::builtin()` 等价于没配皮肤。只对青简渲染器生效。
    pub skin: SkinThemes,
}

/// Router 只产出帧，画交给它；Windows 上由 UI 线程实现。
pub trait CandidateSink: Send {
    /// 把候选窗口摆到 `rect`（组句范围的屏幕矩形）下方并按 `frame` 重绘。
    fn show(&self, frame: Frame, rect: ScreenRect);

    fn hide(&self);

    /// 换画法：装上时、配置或皮肤文件热加载后调，只在设置变了时调。
    fn configure(&self, settings: RenderSettings);
}

/// 不画候选窗口的空实现。
pub struct NoopSink;

impl CandidateSink for NoopSink {
    fn show(&self, _frame: Frame, _rect: ScreenRect) {}

    fn hide(&self) {}

    fn configure(&self, _settings: RenderSettings) {}
}
