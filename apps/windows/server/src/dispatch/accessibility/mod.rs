//! 钉钉 Qt 聊天框的异步上下文兜底；COM 对象只在独立 MTA 线程中使用。

mod pending;
mod reader;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use qingjian_core::SurroundingText;
use qingjian_platform::protocol::SessionId;

pub(super) use pending::Pending;

// 提供方卡住时最多留一个工作线程，不能每段组句再堆一个。
static BUSY: AtomicBool = AtomicBool::new(false);

pub(super) fn request(
    session: SessionId,
    app: Option<&str>,
    before: usize,
    after: usize,
) -> Option<Pending> {
    if !app.is_some_and(|app| app.eq_ignore_ascii_case("DingTalk.exe")) {
        return None;
    }
    let (window, process) = reader::target()?;
    if BUSY.swap(true, Ordering::AcqRel) {
        return None;
    }
    let (send, receive) = mpsc::sync_channel::<Option<SurroundingText>>(1);
    let started = Instant::now();
    let spawn = std::thread::Builder::new()
        .name("qingjian-context".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(|| unsafe {
                CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
                let result = reader::read(window, process, before, after);
                CoUninitialize();
                result
            })
            .ok()
            .flatten();
            let _ = send.send(result);
            BUSY.store(false, Ordering::Release);
        });
    if spawn.is_err() {
        BUSY.store(false, Ordering::Release);
        return None;
    }
    Some(Pending {
        session,
        window,
        started,
        receive,
    })
}
