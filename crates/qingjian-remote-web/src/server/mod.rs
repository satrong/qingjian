//! 监听线程与路由：起一个 socket，把文本经通道交给壳，等壳的回答，再把结果翻译成 HTTP 状态给页面。
//!
//! 线程模型刻意简单：一个 accept 线程（非阻塞 + 50 ms 轮询，所以 drop 掉 [`Server`] 最多半秒就收工，
//! 不用「连自己一下把 accept 唤醒」那类技巧），每条连接一个短命线程。
//! 这里没有任何平台 API——收到文本后能插进哪里，壳说了算。

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::config::Config;
use crate::error::{Error, Outcome};
use crate::http::{self, REQUEST_TIMEOUT, Request, TOKEN_HEADER};
use crate::page;
use crate::text;

/// accept 轮询间隔：定这个值是为了 `Drop` 的最坏等待时间。
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 限流窗口长度，与 [`Config::max_commits_per_minute`] 配套。
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// 交给壳的一次上屏请求：文本，加上一个「你把结果填回来」的把手。
///
/// 壳在哪个线程处理都行，但必须在 [`Config::response_timeout`] 内调用 [`Incoming::complete`]，
/// 否则手机那边看到的是「电脑没响应」。丢弃不答也等价于超时。
pub struct Incoming {
    text: String,
    reply: Sender<Outcome>,
}

impl Incoming {
    /// 要上屏的文本，已经过 [`crate::sanitize`]。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 回一个结果给手机。发送失败只说明手机已经走了（页面被关掉），不用管。
    pub fn complete(self, outcome: Outcome) {
        let _ = self.reply.send(outcome);
    }
}

/// 一个跑着的服务。丢进 [`Option`]、或直接 drop 就关闭（accept 线程随之退出，socket 释放）。
///
/// 平台侧大致是这样用：
///
/// ```no_run
/// # use std::sync::mpsc;
/// # use qingjian_remote_web::{Config, Error, Outcome, Server};
/// # fn demo() -> Result<(), Error> {
/// let (sink, inbox) = mpsc::channel();
/// let server = Server::start(Config { token: Config::generate_token(), ..Config::default() }, sink)?;
///
/// // 壳自己的线程从 inbox 里取，用自己的方式插进当前输入框
/// std::thread::spawn(move || {
///     while let Ok(request) = inbox.recv() {
///         let text = request.text().to_string();
///         let outcome = insert_into_focused_field(&text).unwrap_or(Outcome::NoTarget);
///         request.complete(outcome);
///     }
/// });
/// # let _ = &server;
/// # Ok(())
/// # }
/// # fn insert_into_focused_field(_: &str) -> Result<Outcome, ()> { Ok(Outcome::Committed) }
/// ```
pub struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
    config: Arc<Config>,
    active: Arc<AtomicUsize>,
}

impl Server {
    /// 开始监听。`port` 为 `0` 时让系统挑一个空闲端口（测试用）。
    ///
    /// `sink` 的接收端由壳保管：一旦它被丢掉，提交会得到 `503` 而不是一直挂着。
    /// 起不来（端口被占、没有权限）时返回 [`Error`]，调用方只记日志，不该让输入法出问题。
    pub fn start(config: Config, sink: Sender<Incoming>) -> Result<Self, Error> {
        config.validate().map_err(Error::Invalid)?;

        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.port)))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;

        let active = Arc::new(AtomicUsize::new(0));
        let context = Arc::new(Context {
            config: Arc::new(config),
            sink,
            limiter: Mutex::new((Instant::now(), 0)),
            active: Arc::clone(&active),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let accept = {
            let (context, stop) = (Arc::clone(&context), Arc::clone(&stop));
            thread::Builder::new()
                .name("qingjian-remote-accept".to_string())
                .spawn(move || accept_loop(&listener, &context, &stop))?
        };

        info!(port, "手机输入服务已启动");
        Ok(Self {
            port,
            stop,
            accept: Some(accept),
            config: Arc::clone(&context.config),
            active,
        })
    }

    /// 实际监听的端口。配了 `0` 时这里才是真正拿到的那个。
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 本次服务的配置（含令牌），菜单里显示二维码与地址时用它。
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 正在处理的连接数，测试用来等请求跑完。
    pub fn active_connections(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

/// accept 线程与每条连接共用的东西。
struct Context {
    config: Arc<Config>,
    sink: Sender<Incoming>,
    limiter: Mutex<(Instant, u32)>,
    active: Arc<AtomicUsize>,
}

/// accept 循环：非阻塞 accept + 轮询，`stop` 置位后最多一个轮询周期退出。
fn accept_loop(listener: &TcpListener, context: &Arc<Context>, stop: &AtomicBool) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if context.active.load(Ordering::Acquire) >= context.config.max_connections {
                    let mut stream = stream;
                    let _ = http::respond(&mut stream, 503, "忙，请稍后再试");
                    debug!(%peer, "连接数已满，直接拒绝");
                    continue;
                }
                context.active.fetch_add(1, Ordering::AcqRel);
                let per_connection = Arc::clone(context);
                let spawned = thread::Builder::new()
                    .name("qingjian-remote-conn".to_string())
                    .spawn(move || serve(stream, &per_connection));
                if spawned.is_err() {
                    // 起线程失败就自己把名额还回去，别泄漏计数。
                    context.active.fetch_sub(1, Ordering::AcqRel);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(POLL_INTERVAL);
            }
            Err(error) => {
                // 监听 socket 少见地出错（fd 被关了之类）：歇一下再试，别把 accept 线程转成忙等。
                warn!(%error, "accept 失败");
                thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

/// 一条连接：设超时、读一个请求、回一个响应。连接线程不共享状态，错了只丢日志。
fn serve(mut stream: TcpStream, context: &Arc<Context>) {
    // 监听 socket 是非阻塞的（见 accept 循环），而 Darwin 上 accept 出来的连接会继承这个标记；
    // 不显式改回阻塞，大 body 的续读会立刻拿到 EAGAIN。Linux 不继承，这里设了也无害。
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(REQUEST_TIMEOUT));
    let _ = stream.set_write_timeout(Some(REQUEST_TIMEOUT));

    let guard = ActiveGuard {
        active: &context.active,
    };
    let response = match http::read_request(&mut stream) {
        Ok(request) => {
            debug!(method = %request.method, path = %request.path, "收到请求");
            route(&request, context)
        }
        Err(error) => {
            debug!(%error, "请求不合法");
            Response::text(status_of(&error), message_of(&error))
        }
    };
    let written = if response.html {
        http::respond_html(&mut stream, response.status, &response.body)
    } else {
        http::respond(&mut stream, response.status, &response.body)
    };
    if let Err(error) = written {
        debug!(%error, "写响应失败");
    }
    drop(guard);
}

/// 连接结束时把计数放回去。
struct ActiveGuard<'a> {
    active: &'a AtomicUsize,
}

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 一个待写出的响应。页面（`html`）与纯文本只有 `Content-Type` 的差别。
struct Response {
    status: u16,
    body: String,
    html: bool,
}

impl Response {
    fn text(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            html: false,
        }
    }

    fn page(body: &'static str) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            html: true,
        }
    }
}

/// 三个路由，其余一律 404；路径对方法不对给 405。
fn route(request: &Request, context: &Context) -> Response {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => Response::page(page::PAGE),
        ("GET", "/status") => Response::text(200, "ok"),
        ("POST", "/commit") => commit(request, context),
        ("GET" | "POST", "/" | "/status" | "/commit") => Response::text(405, "这个地址的方法不对"),
        _ => Response::text(404, "没有这个页面"),
    }
}

/// 一次提交：先验来源与令牌，再限流、清洗，最后交给壳。
fn commit(request: &Request, context: &Context) -> Response {
    // 跨源页面带不了自定义令牌头，但浏览器仍可能把请求发过来（不带预检结果的场景）；
    // 这里再核一次 Origin，挡住「网页里嵌一个 form 或 fetch 直发」的情况。
    if let Some(origin) = request.header("origin")
        && !same_authority(origin, request.header("host").unwrap_or_default())
    {
        return Response::text(401, "只接受来自这一页的提交");
    }

    match request.header(TOKEN_HEADER) {
        Some(token) if context.config.token_matches(token) => {}
        _ => return Response::text(401, "令牌不对：请重新扫码配对"),
    }

    if !context.allow_commit() {
        return Response::text(429, "提交太频繁，请稍后再试");
    }

    let raw = match std::str::from_utf8(&request.body) {
        Ok(raw) => raw,
        Err(_) => return Response::text(400, "文本不是 UTF-8"),
    };
    if raw.len() > context.config.max_text_bytes {
        let limit = context.config.max_text_bytes;
        return Response::text(413, format!("文本太长了（上限 {limit} 字节）"));
    }
    let text = match text::sanitize(raw) {
        Ok(text) => text,
        Err(message) => return Response::text(400, message),
    };

    let outcome = match forward(context, text) {
        Ok(outcome) => outcome,
        Err(Error::Timeout) => return Response::text(503, "电脑端没有响应（可能已休眠）"),
        Err(Error::SinkClosed) => return Response::text(503, "手机输入功能已关闭"),
        Err(error) => {
            warn!(%error, "提交失败");
            return Response::text(500, "提交失败");
        }
    };
    match outcome {
        Outcome::Committed => Response::text(200, "已上屏"),
        Outcome::Queued => Response::text(200, "已收到，光标可用时上屏"),
        Outcome::NoTarget => Response::text(409, "电脑端现在没有可以输入的窗口"),
        Outcome::Rejected(reason) => Response::text(409, reason),
    }
}

/// 把文本交给壳并等它的回答。
fn forward(context: &Context, text: String) -> Result<Outcome, Error> {
    let (reply, inbox) = mpsc::channel();
    context
        .sink
        .send(Incoming { text, reply })
        .map_err(|_| Error::SinkClosed)?;
    // 刻意带超时地等：壳的线程若被自己那边卡住，手机不能陪着一起挂。
    match inbox.recv_timeout(context.config.response_timeout) {
        Ok(outcome) => Ok(outcome),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(Error::Timeout),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::SinkClosed),
    }
}

impl Context {
    /// 限流：固定窗口计数。够用，且不用引依赖。
    fn allow_commit(&self) -> bool {
        if self.config.max_commits_per_minute == 0 {
            return true;
        }
        let Ok(mut window) = self.limiter.lock() else {
            return false;
        };
        let (start, count) = *window;
        let now = Instant::now();
        if now.duration_since(start) >= RATE_WINDOW {
            *window = (now, 1);
            return true;
        }
        if count >= self.config.max_commits_per_minute {
            return false;
        }
        window.1 = count + 1;
        true
    }
}

/// `Origin` 与 `Host` 是否指同一台机器的同一个端口。只比 authority，不比协议。
fn same_authority(origin: &str, host: &str) -> bool {
    fn authority(url: &str) -> &str {
        url.split_once("://")
            .map_or(url, |(_, rest)| rest)
            .trim_end_matches('/')
    }
    !host.is_empty() && authority(origin).eq_ignore_ascii_case(host)
}

/// 解析错误 → HTTP 状态与给用户看的话。
fn status_of(error: &Error) -> u16 {
    match error {
        Error::BadRequest(_) => 400,
        Error::TooLarge(_) => 413,
        Error::Timeout => 503,
        Error::SinkClosed => 503,
        // 读超时：手机上发到一半切走了、或网络断在半路，重发一次通常就好。
        Error::Io(io)
            if matches!(
                io.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            408
        }
        Error::Io(_) => 500,
        Error::Invalid(_) => 500,
    }
}

fn message_of(error: &Error) -> String {
    match error {
        Error::BadRequest(_) | Error::TooLarge(_) => error.to_string(),
        Error::Timeout => "电脑端没有响应".to_string(),
        Error::SinkClosed => "手机输入功能已关闭".to_string(),
        Error::Io(io)
            if matches!(
                io.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            "请求发了一半就断了，请再发一次".to_string()
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;
