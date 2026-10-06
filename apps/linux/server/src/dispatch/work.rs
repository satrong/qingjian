//! 工人队列里的一件活：插件的一条请求，或手机推送来的一段文本。

use std::sync::mpsc::Sender;

/// 主线程队列里的一件活。socket 服务与手机推送的泵线程都往这里投，Router 只在工人线程上跑。
pub enum Work {
    /// 插件的一条请求（JSON 值 + 回结果的通道）。队列容量有限，损坏客户端不能无限占用内存。
    Request(serde_json::Value, Sender<Option<serde_json::Value>>),

    /// 手机推送来的一段文本；Router 挑好目标会话后答复它（`Incoming::complete`）。
    Remote(qingjian_remote_web::Incoming),
}
