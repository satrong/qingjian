//! 手机推送上屏：服务按配置起停、真 HTTP 提交进来、目标会话的挑法、文本捎给 DLL 的时机。
//!
//! 走的是完整链路（`qingjian-remote-web` 的服务 → 泵线程 → [`Work::Remote`] → Router），
//! 只是把管道工人循环换成本文件里的 `recv` 循环：DLL 那半边要 Windows 机器才能验。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::Duration;

use qingjian_platform::RemoteConfig;
use qingjian_platform::protocol::{ClientMessage, ServerMessage};

use super::support::*;
use qingjian_windows_server::ipc::Work;

/// 测试用的配置：端口 0 让系统挑空闲端口，免得跟真机上跑着的那个撞。
fn config(enabled: bool) -> RemoteConfig {
    RemoteConfig {
        enabled,
        port: 0,
        token: "test-token".to_string(),
    }
}

/// 开着服务，返回（实际监听的端口、工作通道的接收端——泵线程把文本投到这里）。
fn start(router: &mut Router) -> (u16, mpsc::Receiver<Work>) {
    let (work, inbox) = mpsc::channel::<Work>();
    router.set_work_sender(work);
    router.sync_remote(&config(true));
    (router.remote_port().expect("服务已开"), inbox)
}

/// 从 `127.0.0.1:<port>` 发一次提交，返回 HTTP 状态码。
fn post(port: u16, token: &str, text: &str) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连上服务");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let request = format!(
        "POST /commit HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Qingjian-Token: {token}\r\n\
         Content-Type: text/plain;charset=utf-8\r\nContent-Length: {}\r\n\r\n{text}",
        text.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("读完响应");
    response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("响应里没有状态码：{response}"))
}

/// 替 DLL 的位置：把工作通道里的活跑掉，直到拿到 `Work::Remote`（或超时）。
fn pump_once(router: &mut Router, inbox: &mpsc::Receiver<Work>) -> bool {
    match inbox.recv_timeout(Duration::from_secs(5)) {
        Ok(Work::Remote(incoming)) => {
            router.accept_remote(incoming);
            true
        }
        Ok(_) => false,
        Err(error) => panic!("没等到推送：{error}"),
    }
}

#[test]
fn off_by_default_until_the_config_turns_it_on() {
    let mut router = router();
    let (_work, _inbox) = mpsc::channel::<Work>();
    router.set_work_sender(_work);
    router.sync_remote(&config(false));
    assert_eq!(router.remote_port(), None);
    router.sync_remote(&config(true));
    let port = router.remote_port().expect("开着就该有端口");
    // 再同步一次同样的配置不该重启服务（端口不变）
    router.sync_remote(&config(true));
    assert_eq!(router.remote_port(), Some(port));
    // 关掉：端口释放
    router.sync_remote(&config(false));
    assert_eq!(router.remote_port(), None);
}

#[test]
fn without_the_worker_channel_the_service_stays_off() {
    // 没接管道服务时（Engine 侧测试）不该起服务：文本没地方投
    let mut router = router();
    router.sync_remote(&config(true));
    assert_eq!(router.remote_port(), None);
}

#[test]
fn a_push_lands_in_the_open_session_and_rides_the_next_reply() {
    let mut router = router();
    let (port, inbox) = start(&mut router);

    let client = std::thread::spawn(move || post(port, "test-token", "手机来的一句话"));
    assert!(pump_once(&mut router, &inbox));
    assert_eq!(client.join().unwrap(), 200);

    // 组句期间的 Update（Poll 的应答）捎给它
    let Some(ServerMessage::Update { remote, .. }) =
        router.handle(ClientMessage::Poll { session: SESSION })
    else {
        panic!("poll 应答 Update");
    };
    assert_eq!(remote.as_deref(), Some("手机来的一句话"));

    // 取走就没了：下一拍不再带
    let Some(ServerMessage::ModeSync { remote, .. }) =
        router.handle(ClientMessage::SyncMode { session: SESSION })
    else {
        panic!("sync 应答 ModeSync");
    };
    assert_eq!(remote, None);
}

#[test]
fn the_idle_path_is_mode_sync() {
    // 没在组句时 DLL 不收 Poll，只收 SyncMode（320 ms 一次），所以这一路也得能带到
    let mut router = router();
    let (port, inbox) = start(&mut router);
    let client = std::thread::spawn(move || post(port, "test-token", "空闲时来的"));
    assert!(pump_once(&mut router, &inbox));
    assert_eq!(client.join().unwrap(), 200);
    let Some(ServerMessage::ModeSync { remote, .. }) =
        router.handle(ClientMessage::SyncMode { session: SESSION })
    else {
        panic!("sync 应答 ModeSync");
    };
    assert_eq!(remote.as_deref(), Some("空闲时来的"));
}

#[test]
fn a_wrong_token_never_reaches_the_router() {
    let mut router = router();
    let (port, inbox) = start(&mut router);
    assert_eq!(post(port, "wrong", "不该到"), 401);
    // 没有活可做：Router 里不该攒下文本
    let Some(ServerMessage::ModeSync { remote, .. }) =
        router.handle(ClientMessage::SyncMode { session: SESSION })
    else {
        panic!("sync 应答 ModeSync");
    };
    assert_eq!(remote, None);
    assert!(inbox.try_recv().is_err());
}

#[test]
fn a_private_session_is_never_the_target() {
    let mut router = router();
    // 报私密（密码框）：没有可插入的窗口，手机那边拿到的是 409
    router.handle(ClientMessage::Privacy {
        session: SESSION,
        private: true,
    });
    let (port, inbox) = start(&mut router);
    let client = std::thread::spawn(move || post(port, "test-token", "不该插进密码框"));
    assert!(pump_once(&mut router, &inbox));
    assert_eq!(client.join().unwrap(), 409);
    let Some(ServerMessage::ModeSync { remote, .. }) =
        router.handle(ClientMessage::SyncMode { session: SESSION })
    else {
        panic!("sync 应答 ModeSync");
    };
    assert_eq!(remote, None);
}

#[test]
fn a_push_clears_the_composition_first() {
    let mut router = router();
    // 先打两个字进组句
    press(&mut router, letter('n'));
    press(&mut router, letter('i'));
    let (port, inbox) = start(&mut router);
    let client = std::thread::spawn(move || post(port, "test-token", "手机来的一句话"));
    assert!(pump_once(&mut router, &inbox));
    assert_eq!(client.join().unwrap(), 200);
    // 组句已被清掉：捎来的文本是直接上屏，不替换拼音行
    let Some(ServerMessage::Update { frame, remote, .. }) =
        router.handle(ClientMessage::Poll { session: SESSION })
    else {
        panic!("poll 应答 Update");
    };
    assert_eq!(remote.as_deref(), Some("手机来的一句话"));
    assert!(frame.preedit.is_empty(), "组句应当已清空：{frame:?}");
}
