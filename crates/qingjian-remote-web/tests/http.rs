//! 真起一个监听、用真的 TCP 打一遍：这是手机浏览器会走的完整路径。
//! 单元测试覆盖的是解析与路由的分支，这里确认拼起来能用（绑定、线程、状态码、响应体）。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use qingjian_remote_web::{Config, Incoming, Outcome, Server};

/// 起一个服务并把通道的接收端交给测试：测试扮演壳，自己收文本、自己回答复。
fn serve() -> (u16, Receiver<Incoming>, Server) {
    let (sink, inbox) = mpsc::channel();
    let server = Server::start(
        Config {
            port: 0,
            token: "test-token".to_string(),
            ..Config::default()
        },
        sink,
    )
    .expect("起服务");
    (server.port(), inbox, server)
}

/// 发一个原始请求，返回 (状态码, body)。
fn request(port: u16, raw: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连上服务");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("读完响应");
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("响应里没有状态码：{response}"));
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .to_string();
    (status, body)
}

fn get(port: u16, path: &str) -> (u16, String) {
    request(
        port,
        &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
    )
}

fn commit(port: u16, token: &str, text: &str) -> (u16, String) {
    request(
        port,
        &format!(
            "POST /commit HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Qingjian-Token: {token}\r\n\
             Content-Type: text/plain;charset=utf-8\r\nContent-Length: {}\r\n\r\n{text}",
            text.len()
        ),
    )
}

#[test]
fn serves_the_page_and_hands_text_to_the_shell() {
    let (port, inbox, _server) = serve();

    let (status, body) = get(port, "/");
    assert_eq!(status, 200);
    assert!(body.contains("<textarea"), "首页要含输入框");

    let client = thread::spawn(move || commit(port, "test-token", "你好，世界。"));
    let incoming = inbox
        .recv_timeout(Duration::from_secs(2))
        .expect("壳收到提交");
    assert_eq!(incoming.text(), "你好，世界。");
    incoming.complete(Outcome::Committed);

    let (status, body) = client.join().unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, "已上屏");
}

#[test]
fn a_missing_window_is_reported_as_409() {
    let (port, inbox, _server) = serve();

    let client = thread::spawn(move || commit(port, "test-token", "你好"));
    inbox
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .complete(Outcome::NoTarget);

    let (status, body) = client.join().unwrap();
    assert_eq!(status, 409);
    assert!(body.contains("没有可以输入的窗口"));
}

#[test]
fn a_silent_shell_times_out() {
    let (port, inbox, _server) = serve();

    let started = std::time::Instant::now();
    let (status, body) = commit(port, "test-token", "你好");
    assert_eq!(status, 503);
    assert!(body.contains("没有响应"));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "要按 response_timeout 收工"
    );
    // 壳没答，请求仍然到了它手上（只是没人回）。
    assert_eq!(
        inbox.try_recv().map(|r| r.text().to_string()).ok(),
        Some("你好".to_string())
    );
}

#[test]
fn refuses_a_wrong_token_and_an_unknown_path() {
    let (port, inbox, _server) = serve();

    assert_eq!(commit(port, "wrong", "你好").0, 401);
    assert_eq!(get(port, "/nope").0, 404);
    assert!(inbox.try_recv().is_err(), "被拒的提交不该到壳");
}

#[test]
fn answers_status() {
    let (port, _inbox, _server) = serve();
    assert_eq!(get(port, "/status"), (200, "ok".to_string()));
}

#[test]
fn commits_a_body_larger_than_one_read() {
    // 一千字（3000 字节）比 socket 单次读的 1 KiB 大，走的是「边读边续」那条路径。
    let (port, inbox, _server) = serve();
    let long = "字".repeat(1000);
    let client = thread::spawn(move || commit(port, "test-token", &long));

    let incoming = inbox
        .recv_timeout(Duration::from_secs(2))
        .expect("壳收到提交");
    assert_eq!(incoming.text().len(), 3000);
    incoming.complete(Outcome::Committed);

    assert_eq!(client.join().unwrap().0, 200);
}

#[test]
fn rejects_an_oversized_body() {
    let (port, inbox, _server) = serve();
    let long = "字".repeat(Config::default().max_text_bytes);
    let (status, body) = commit(port, "test-token", &long);
    assert_eq!(status, 413, "body: {body}");
    assert!(inbox.try_recv().is_err());
}

#[test]
fn dropping_the_server_closes_the_port() {
    let (port, _inbox, server) = serve();
    assert_eq!(get(port, "/status").0, 200);
    drop(server);
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "drop 之后端口应当释放"
    );
}
