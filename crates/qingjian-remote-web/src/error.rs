//! 错误类型：起服务时的失败、解析请求时的失败、以及等平台侧答复时的失败。
//!
//! `#[error]` 里的文案是英文（日志与诊断用）；给手机页面看的中文提示在 `server::commit` 里另行映射。

use std::io;

/// 起服务或处理请求时出错。都只记日志，不该让输入法崩掉。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("listen or read/write failed: {0}")]
    Io(#[from] io::Error),

    #[error("invalid config: {0}")]
    Invalid(&'static str),

    #[error("malformed request: {0}")]
    BadRequest(&'static str),

    #[error("body is over the {0} byte limit")]
    TooLarge(usize),

    #[error("the shell stopped accepting submissions")]
    SinkClosed,

    #[error("the shell did not answer in time")]
    Timeout,
}

/// 壳对一次上屏请求的回答，经 [`crate::Incoming::complete`] 回给服务，服务据此给页面一个 HTTP 状态。
///
/// 三个变体对页面是同一类失败（`409`），区别只在给用户看的话。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 已经插进电脑端的输入框。
    Committed,

    /// 已经收下，但要等输入框可用时才插得进去（macOS 上推送到达时可能没有活动 client）。
    /// 手机上仍算成功，只是要晚一点出现。
    Queued,

    /// 眼下没有能接收文本的窗口：没有活动输入框，或光标不在文本控件上。
    NoTarget,

    /// 壳主动拒绝，并给出原因（例如处于安全输入/密码框）。
    Rejected(&'static str),
}
