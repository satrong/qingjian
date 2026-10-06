use std::sync::mpsc::Sender;

use qingjian_platform::protocol::{ClientMessage, ServerMessage};

use crate::dispatch::StatusEvent;

/// 手机推送来的一次上屏请求（由 `qingjian-remote-web` 的泵线程投进来）。
pub type RemoteText = qingjian_remote_web::Incoming;

/// 工人线程（独占 [`crate::dispatch::Router`]）的一件活：DLL 的一条消息，或状态条上的一次操作。
pub enum Work {
    /// 某条连接收到的消息 + 回结果的通道（`None` 表示不用回话）。
    Client(ClientMessage, Sender<Option<ServerMessage>>),

    /// UI 线程发来的状态条操作。
    Status(StatusEvent),

    /// 手机推送来的一段文本；Router 挑好目标会话后答复它（`Incoming::complete`）。
    Remote(RemoteText),
}
