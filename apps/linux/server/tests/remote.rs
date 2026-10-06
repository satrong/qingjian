//! 手机推送上屏：服务按配置起停、真 HTTP 提交进来、目标会话的挑法、文本捎在 KeyResult 上。
//!
//! 走完整链路（`qingjian-remote-web` 的服务 → 泵线程 → [`Work::Remote`] → Router → fcitx 插件会看到的
//! 回包）；插件那半边是 C++，要 Linux 上才验得了。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::Duration;

use qingjian_core::Engine;
use qingjian_dictionary::Dictionary;
use qingjian_linux_server::dispatch::Work;
use qingjian_linux_server::protocol::{LinuxEvent, LinuxRequest};
use qingjian_linux_server::{Router, RouterConfig};
use qingjian_platform::RemoteConfig;
use qingjian_platform::protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage, SessionId};

const SESSION: SessionId = SessionId(1);

fn router() -> Router {
    let dictionary =
        Dictionary::parse("你好\tni hao\t100\n开发\tkai fa\t90\n你\tni\t80\n好\thao\t80\n")
            .unwrap();
    Router::new(Engine::new(dictionary), RouterConfig::default())
}

/// 开一个会话、报上能力、报焦点：插件插话之前必须走到这三步，否则 Server 不认这个会话。
fn open_active(router: &mut Router) {
    router.handle(ClientMessage::OpenSession {
        session: SESSION,
        app: Some("test".into()),
        protocol: PROTOCOL_VERSION,
    });
    for value in [json_caps(false), json_focus(true)] {
        let _ = event(router, value);
    }
}

fn json_caps(private: bool) -> LinuxEvent {
    serde_json::from_value(serde_json::json!({
        "Capabilities": { "password": private, "sensitive": private, "disabled": false }
    }))
    .expect("能力事件")
}

fn json_focus(focused: bool) -> LinuxEvent {
    serde_json::from_value(serde_json::json!({ "Focus": { "focused": focused } }))
        .expect("焦点事件")
}

fn config(enabled: bool) -> RemoteConfig {
    RemoteConfig {
        enabled,
        port: 0,
        token: "test-token".to_string(),
    }
}

/// 起服务，返回（端口、工作通道接收端）。
fn start(router: &mut Router) -> (u16, mpsc::Receiver<Work>) {
    let (work, inbox) = mpsc::sync_channel::<Work>(8);
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

/// 替插件的位置：把工作通道里的活跑掉。
fn pump_once(router: &mut Router, inbox: &mpsc::Receiver<Work>) {
    match inbox.recv_timeout(Duration::from_secs(5)) {
        Ok(Work::Remote(incoming)) => router.accept_remote(incoming),
        Ok(_) => {}
        Err(error) => panic!("没等到推送：{error}"),
    }
}

/// 让插件「动一下」：发一个事件，拿回包。
fn event(router: &mut Router, event: LinuxEvent) -> Option<ServerMessage> {
    // 线上格式：{"LinuxEvent": {"session": .., "event": ..}}
    let request = serde_json::json!({
        "LinuxEvent": serde_json::to_value(LinuxRequest { session: SESSION, event })
            .expect("事件封套"),
    });
    router.handle_linux(request).and_then(|value| {
        // 回包整体就是外部标签的枚举（插件那边读 response.at("KeyResult")）
        let text = value.to_string();
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}；回包={text}"))
    })
}

#[test]
fn off_until_the_config_says_otherwise() {
    let mut router = router();
    let (work, _inbox) = mpsc::sync_channel::<Work>(8);
    router.set_work_sender(work);
    router.sync_remote(&config(false));
    assert_eq!(router.remote_port(), None);
    router.sync_remote(&config(true));
    let port = router.remote_port().expect("开着就该有端口");
    router.sync_remote(&config(true));
    assert_eq!(router.remote_port(), Some(port), "同样的配置不该重启服务");
    router.sync_remote(&config(false));
    assert_eq!(router.remote_port(), None);
}

#[test]
fn a_push_rides_the_next_event_and_only_once() {
    let mut router = router();
    open_active(&mut router);
    let (port, inbox) = start(&mut router);

    let client = std::thread::spawn(move || post(port, "test-token", "手机来的一句话"));
    pump_once(&mut router, &inbox);
    assert_eq!(client.join().unwrap(), 200);

    let Some(ServerMessage::KeyResult { remote, .. }) = event(&mut router, json_focus(true)) else {
        panic!("事件回包 KeyResult");
    };
    assert_eq!(remote.as_deref(), Some("手机来的一句话"));
    // 取走就没了：下一个事件不再带
    let Some(ServerMessage::KeyResult { remote, .. }) = event(&mut router, json_focus(true)) else {
        panic!("事件回包 KeyResult");
    };
    assert_eq!(remote, None);
}

#[test]
fn a_private_session_is_never_the_target() {
    let mut router = router();
    open_active(&mut router);
    // 密码框：能力里报 password
    let _ = event(&mut router, json_caps(true));
    let (port, inbox) = start(&mut router);
    let client = std::thread::spawn(move || post(port, "test-token", "不该插进密码框"));
    pump_once(&mut router, &inbox);
    assert_eq!(client.join().unwrap(), 409);
    let Some(ServerMessage::KeyResult { remote, .. }) = event(&mut router, json_focus(true)) else {
        panic!("事件回包 KeyResult");
    };
    assert_eq!(remote, None);
}

#[test]
fn a_wrong_token_never_reaches_the_router() {
    let mut router = router();
    open_active(&mut router);
    let (port, inbox) = start(&mut router);
    assert_eq!(post(port, "wrong", "不该到"), 401);
    assert!(inbox.try_recv().is_err());
    let Some(ServerMessage::KeyResult { remote, .. }) = event(&mut router, json_focus(true)) else {
        panic!("事件回包 KeyResult");
    };
    assert_eq!(remote, None);
}

#[test]
fn a_push_clears_the_composition_first() {
    let mut router = router();
    open_active(&mut router);
    // 打两个字进组句
    for c in ['n', 'i'] {
        router.handle(ClientMessage::Key {
            session: SESSION,
            event: qingjian_platform::protocol::KeyEvent::new(
                c as u32,
                Some(c),
                qingjian_platform::protocol::KeyModifiers::default(),
            ),
        });
    }
    let (port, inbox) = start(&mut router);
    let client = std::thread::spawn(move || post(port, "test-token", "手机来的一句话"));
    pump_once(&mut router, &inbox);
    assert_eq!(client.join().unwrap(), 200);
    let Some(ServerMessage::KeyResult { frame, remote, .. }) = event(&mut router, json_focus(true))
    else {
        panic!("事件回包 KeyResult");
    };
    assert_eq!(remote.as_deref(), Some("手机来的一句话"));
    assert!(frame.preedit.is_empty(), "组句应当已清空：{frame:?}");
}
