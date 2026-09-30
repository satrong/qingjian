//! 服务端关闭空闲连接后，后台 worker 必须在等待输入期间处理断连。

use std::thread;
use std::time::{Duration, Instant};

use qingjian_core::{Prediction, PredictionKind, PredictionRequest, Predictor};
use qingjian_predict::CloudPredictor;

mod support;

#[test]
fn prediction_reconnects_after_server_closes_idle_connection() {
    reconnect(0);
}

#[test]
fn prediction_reconnects_with_debounce() {
    reconnect(100);
}

fn reconnect(debounce_ms: u64) {
    let (mut config, closed_rx, server) = support::idle_server();
    config.debounce_ms = debounce_ms;
    let mut predictor = CloudPredictor::new(&config).unwrap();
    if debounce_ms > 0 {
        predictor.submit(request(0));
    }
    predictor.submit(request(1));
    let first = wait_prediction(&mut predictor);
    assert_eq!(first.sequence, 1, "防抖只应发送最新请求");
    assert!(!first.failed);
    assert_eq!(first.sentence.as_deref(), Some("我们"));
    closed_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    thread::sleep(Duration::from_millis(200));
    predictor.submit(request(2));
    let result = wait_prediction(&mut predictor);
    assert_eq!(result.sequence, 2);
    assert!(!result.failed, "空闲连接关闭后，下一次联想不应失败");
    server.join().unwrap();

    // 服务已经退出，相同内容仍应从缓存返回，并使用新的请求序号。
    let mut cached = request(1);
    cached.sequence = 3;
    predictor.submit(cached);
    let cached = wait_prediction(&mut predictor);
    assert_eq!(cached.sequence, 3);
    assert!(!cached.failed);
    assert_eq!(cached.sentence, first.sentence);
}

fn request(sequence: u64) -> PredictionRequest {
    PredictionRequest {
        sequence,
        kind: PredictionKind::Compose,
        // 两次请求的缓存键不同，确保第二次确实经过网络。
        before: sequence.to_string(),
        after: String::new(),
        pinyin: "wo'men".into(),
        letters: "women".into(),
        syllables: 2,
        candidates: Vec::new(),
        guess: String::new(),
        max_items: 0,
        want_sentence: true,
        text: String::new(),
        target_language: String::new(),
    }
}

fn wait_prediction(predictor: &mut CloudPredictor) -> Prediction {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(result) = predictor.poll() {
            return result;
        }
        assert!(Instant::now() < deadline, "等待联想结果超时");
        thread::sleep(Duration::from_millis(5));
    }
}
