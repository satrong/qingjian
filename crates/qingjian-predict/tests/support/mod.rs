//! 本地 HTTP 服务：首次回复后关闭空闲连接，第二次只接受新连接。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use qingjian_predict::PredictConfig;

pub fn idle_server() -> (PredictConfig, Receiver<()>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let config = PredictConfig {
        base_url: format!("http://{}/v1", listener.local_addr().unwrap()),
        api_key: Some("test-key".into()),
        debounce_ms: 0,
        timeout_ms: 2000,
        ..PredictConfig::default()
    };
    let (closed_tx, closed_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let mut first = accept(&listener);
        respond(&mut first);
        // 等 worker 交付结果、回到等待输入，再模拟服务端的 keep-alive 超时。
        thread::sleep(Duration::from_millis(100));
        first.shutdown(Shutdown::Both).unwrap();
        drop(first);
        closed_tx.send(()).unwrap();
        let mut second = accept(&listener);
        respond(&mut second);
    });
    (config, closed_rx, server)
}

fn accept(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                return stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "没有收到新的连接");
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("{error}"),
        }
    }
}

fn respond(stream: &mut TcpStream) {
    let mut reader = BufReader::new(&mut *stream);
    let mut length = 0;
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().unwrap();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let user: serde_json::Value =
        serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
    let content = if let Some(words) = user["words"].as_array() {
        serde_json::json!({"items": words.iter().map(|word| serde_json::json!({
            "w": word, "pos": "n.", "senses": [{"t": "test"}]
        })).collect::<Vec<_>>()})
    } else {
        serde_json::json!({"words": [], "sentence": "我们"})
    };
    let body = serde_json::json!({
        "id": "test", "object": "chat.completion", "created": 0, "model": "test",
        "choices": [{"index": 0, "message": {"role": "assistant",
            "content": content.to_string()}, "finish_reason": "stop"}]
    })
    .to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
    .unwrap();
    stream.flush().unwrap();
}
