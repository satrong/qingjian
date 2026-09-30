//! 释义兜底的空闲连接回归：攒批和等待新词期间都必须处理 HTTP 断连。

use std::thread;
use std::time::{Duration, Instant};

use qingjian_core::{GlossFiller, Language};
use qingjian_predict::CloudGlossFiller;

mod support;

#[test]
fn gloss_reconnects_after_server_closes_idle_connection() {
    let (config, closed_rx, server) = support::idle_server();
    let mut filler = CloudGlossFiller::new(&config).unwrap();
    // 第一批立即发出，第二批单词等待攒批超时，覆盖两种请求路径。
    for i in 0..8 {
        filler.request(Language::English, &format!("词{i}"));
    }
    wait_glosses(&mut filler, 8);
    closed_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    thread::sleep(Duration::from_millis(200));
    filler.request(Language::English, "新词");
    wait_glosses(&mut filler, 1);
    server.join().unwrap();
}

fn wait_glosses(filler: &mut CloudGlossFiller, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut received = 0;
    while received < expected {
        received += filler.poll().len();
        assert!(
            Instant::now() < deadline,
            "空闲连接关闭后，释义兜底未返回结果"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(received, expected);
}
