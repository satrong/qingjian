//! 在飞的无障碍读取；组句生命周期结束时直接丢弃，不等待提供方。

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use qingjian_core::SurroundingText;
use qingjian_platform::protocol::SessionId;

pub(in crate::dispatch) struct Pending {
    pub(super) session: SessionId,

    pub(super) window: usize,

    pub(super) started: Instant,

    pub(super) receive: Receiver<Option<SurroundingText>>,
}

impl Pending {
    /// 外层 Option 为 None 表示还没完成，Some(None) 表示本轮失败或失效。
    pub(in crate::dispatch) fn poll(
        &self,
        focused: Option<SessionId>,
    ) -> Option<Option<SurroundingText>> {
        self.poll_at(
            focused,
            super::reader::foreground(self.window),
            Instant::now(),
        )
    }

    pub(super) fn poll_at(
        &self,
        focused: Option<SessionId>,
        foreground: bool,
        now: Instant,
    ) -> Option<Option<SurroundingText>> {
        if focused != Some(self.session)
            || now.duration_since(self.started) > Duration::from_millis(500)
            || !foreground
        {
            return Some(None);
        }
        match self.receive.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(None),
        }
    }
}
