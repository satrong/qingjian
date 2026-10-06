//! `server` 的单测：路由、鉴权、限流、状态映射与壳侧握手。

use super::*;

fn context(token: &str) -> Context {
    Context {
        config: Arc::new(Config {
            token: token.to_string(),
            ..Config::default()
        }),
        sink: mpsc::channel().0,
        limiter: Mutex::new((Instant::now(), 0)),
        active: Arc::new(AtomicUsize::new(0)),
    }
}

/// 带指定通道的 Context，令牌固定为 `token`。
fn context_with(sink: mpsc::Sender<Incoming>, token: &str) -> Context {
    Context {
        config: Arc::new(Config {
            token: token.to_string(),
            ..Config::default()
        }),
        sink,
        limiter: Mutex::new((Instant::now(), 0)),
        active: Arc::new(AtomicUsize::new(0)),
    }
}

fn post(token: &str, body: &str) -> Request {
    let mut headers = vec![("content-length".to_string(), body.len().to_string())];
    if !token.is_empty() {
        headers.push((TOKEN_HEADER.to_string(), token.to_string()));
    }
    Request {
        method: "POST".to_string(),
        path: "/commit".to_string(),
        headers,
        body: body.as_bytes().to_vec(),
    }
}

#[test]
fn serves_the_page_on_get_root() {
    let context = context("t");
    let request = Request {
        method: "GET".to_string(),
        path: "/".to_string(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    let response = route(&request, &context);
    assert_eq!(response.status, 200);
    assert!(response.html);
    assert!(response.body.contains("<textarea"));
}

#[test]
fn reports_status() {
    let context = context("t");
    let request = Request {
        method: "GET".to_string(),
        path: "/status".to_string(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    assert_eq!(route(&request, &context).status, 200);
}

#[test]
fn unknown_paths_are_404_and_wrong_methods_are_405() {
    let context = context("t");
    let get = |path: &str| Request {
        method: "GET".to_string(),
        path: path.to_string(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    assert_eq!(route(&get("/nope"), &context).status, 404);
    assert_eq!(route(&get("/commit"), &context).status, 405);
}

#[test]
fn a_missing_or_wrong_token_is_401() {
    let context = context("secret");
    assert_eq!(route(&post("", "你好"), &context).status, 401);
    assert_eq!(route(&post("wrong", "你好"), &context).status, 401);
}

#[test]
fn a_cross_origin_post_is_rejected() {
    let context = context("secret");
    let mut request = post("secret", "你好");
    request
        .headers
        .push(("origin".to_string(), "http://evil.example".to_string()));
    request
        .headers
        .push(("host".to_string(), "192.168.1.2:23333".to_string()));
    assert_eq!(route(&request, &context).status, 401);
}

#[test]
fn a_same_origin_post_gets_a_verdict_from_the_shell() {
    let (sink, inbox) = mpsc::channel();
    let context = Context {
        config: Arc::new(Config {
            token: "secret".to_string(),
            ..Config::default()
        }),
        sink,
        limiter: Mutex::new((Instant::now(), 0)),
        active: Arc::new(AtomicUsize::new(0)),
    };
    let shell = thread::spawn(move || {
        let request = inbox.recv().unwrap();
        assert_eq!(request.text(), "你好");
        request.complete(Outcome::Committed);
    });

    let response = route(&post("secret", "你好"), &context);
    shell.join().unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "已上屏");
}

#[test]
fn queued_is_a_success_with_its_own_wording() {
    let (sink, inbox) = mpsc::channel();
    let context = context_with(sink, "secret");
    thread::spawn(move || inbox.recv().unwrap().complete(Outcome::Queued));
    let response = route(&post("secret", "你好"), &context);
    assert_eq!(response.status, 200);
    assert!(response.body.contains("光标可用时上屏"));
}

#[test]
fn no_target_and_rejection_both_become_409() {
    let verdicts = [Outcome::NoTarget, Outcome::Rejected("处于安全输入")];
    for verdict in verdicts {
        let (sink, inbox) = mpsc::channel::<Incoming>();
        let context = Context {
            config: Arc::new(Config {
                token: "secret".to_string(),
                ..Config::default()
            }),
            sink,
            limiter: Mutex::new((Instant::now(), 0)),
            active: Arc::new(AtomicUsize::new(0)),
        };
        thread::spawn(move || inbox.recv().unwrap().complete(verdict));
        assert_eq!(route(&post("secret", "你好"), &context).status, 409);
    }
}

#[test]
fn a_silent_shell_times_out_as_503() {
    let (sink, _inbox) = mpsc::channel();
    let context = Context {
        config: Arc::new(Config {
            token: "secret".to_string(),
            response_timeout: Duration::from_millis(50),
            ..Config::default()
        }),
        sink,
        limiter: Mutex::new((Instant::now(), 0)),
        active: Arc::new(AtomicUsize::new(0)),
    };
    assert_eq!(route(&post("secret", "你好"), &context).status, 503);
}

#[test]
fn a_dropped_sink_is_503() {
    let (sink, inbox) = mpsc::channel();
    drop(inbox);
    let context = Context {
        config: Arc::new(Config {
            token: "secret".to_string(),
            ..Config::default()
        }),
        sink,
        limiter: Mutex::new((Instant::now(), 0)),
        active: Arc::new(AtomicUsize::new(0)),
    };
    assert_eq!(route(&post("secret", "你好"), &context).status, 503);
}

#[test]
fn blank_and_oversized_texts_are_refused_before_the_shell() {
    let context = context("secret");
    assert_eq!(route(&post("secret", "   "), &context).status, 400);
    let long = "字".repeat(context.config.max_text_bytes);
    assert_eq!(route(&post("secret", &long), &context).status, 413);
}

#[test]
fn rate_limit_trips_and_recovers_with_the_window() {
    let mut context = context("secret");
    context.config = Arc::new(Config {
        token: "secret".to_string(),
        max_commits_per_minute: 2,
        ..Config::default()
    });
    assert!(context.allow_commit());
    assert!(context.allow_commit());
    assert!(!context.allow_commit());
    // 把窗口起点推到过去，等价于过了一分钟。
    *context.limiter.lock().unwrap() = (Instant::now() - RATE_WINDOW - Duration::from_secs(1), 2);
    assert!(context.allow_commit());
}

#[test]
fn unlimited_when_the_limit_is_zero() {
    let context = Context {
        config: Arc::new(Config {
            token: "secret".to_string(),
            max_commits_per_minute: 0,
            ..Config::default()
        }),
        sink: mpsc::channel().0,
        limiter: Mutex::new((Instant::now(), 0)),
        active: Arc::new(AtomicUsize::new(0)),
    };
    for _ in 0..100 {
        assert!(context.allow_commit());
    }
}

#[test]
fn authority_comparison_ignores_scheme_and_trailing_slash() {
    assert!(same_authority(
        "http://192.168.1.2:23333",
        "192.168.1.2:23333"
    ));
    assert!(same_authority(
        "https://192.168.1.2:23333/",
        "192.168.1.2:23333"
    ));
    assert!(!same_authority("http://evil.example", "192.168.1.2:23333"));
    assert!(!same_authority(
        "http://192.168.1.2:23334",
        "192.168.1.2:23333"
    ));
    assert!(!same_authority("http://192.168.1.2", ""));
}

#[test]
fn errors_map_to_status_and_message() {
    assert_eq!(status_of(&Error::BadRequest("头部区不完整")), 400);
    assert_eq!(status_of(&Error::TooLarge(999)), 413);
    assert!(message_of(&Error::TooLarge(999)).contains("999"));
    assert_eq!(status_of(&Error::Timeout), 503);
}

#[test]
fn a_half_sent_request_reads_as_a_timeout_not_a_parse_error() {
    let timed_out = Error::Io(io::Error::from(io::ErrorKind::WouldBlock));
    assert_eq!(status_of(&timed_out), 408);
    assert!(message_of(&timed_out).contains("再发一次"));
}
