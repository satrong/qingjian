//! 手机推送上屏：服务生命周期、待插入队列与配对面板。
//!
//! 推送到达时**未必有能接收的 client**——输入法开着但光标不在文本框时，`insertText:` 没有收件人。
//! 所以文本收下后先落进待插入队列，等 IMK 下一次拿到 client（`activateServer:` 或任一键事件）时才插进去。
//!
//! 两条线程之间只过两样东西：一条 mpsc 通道（服务 → 泵线程）与一个 `Mutex`（泵线程 → 主线程）。
//! 没有跨线程的 objc 调用：手机那几百毫秒的答复由**泵线程**立即给（`Outcome::Queued`，意思是
//! 「收下了，等光标可用时上屏」，不是「屏幕上已经出现了」），插入要等主线程，答复不能等它。
//!
//! 主线程这一侧有两条插入路径。IMK 回调（`activateServer:` / 任一键事件）手上就有 client，直接插；
//! 但推送到达时用户可能正好没在打字，没有回调。所以服务开着时另有一个 200 ms 的主线程定时器
//! （[`RemoteInput::start_timer`]）——会话开始时存下的 client 这时能用上，就立刻插；
//! 没有会话（没点在任何输入框上）就把文本留着，等下一次回调。
//!
//! 超过 [`PENDING_TTL`] 还没等到 client 就丢掉，避免很久以后插到不相干的地方。
//!
//! 设计见 `docs/design/remote-input.md`。

mod panel;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};

use objc2_foundation::{NSObject, NSRunLoop, NSRunLoopCommonModes, NSTimer};
use qingjian_platform::{DEFAULT_REMOTE_PORT, RemoteConfig};
use qingjian_remote_web::{Config as ServiceConfig, Incoming, Outcome, Server, primary_lan_ip};

use panel::RemotePanel;

/// 待插入文本能等多久。等不到就丢：宁可不插，也不要过很久以后插到不相干的地方。
const PENDING_TTL: Duration = Duration::from_secs(30);

/// 主线程定时器的间隔：醒来问一次「当前有没有可插入的输入框」。
const TICK_INTERVAL: f64 = 0.2;

define_class!(
    // SAFETY: NSObject 没有子类化要求；定时器持有它，RemoteInput 也持有一份。
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    /// 定时器的 target：醒来问 Host 有没有攒着的文本可以插。
    pub struct Ticker;

    impl Ticker {
        #[unsafe(method(tickRemoteInput:))]
        fn tick_remote_input(&self, _timer: Option<&NSTimer>) {
            crate::host::with(|host| host.flush_remote_now());
        }
    }
);

unsafe impl objc2::runtime::NSObjectProtocol for Ticker {}

/// 手机推送上屏。归 [`crate::host::Host`] 所有，方法都在主线程被 IMK 回调、菜单动作或定时器调到。
///
/// 服务交文本过来的通道由一条**泵线程**接着：它把文本挪进 [`Self::pending`] 并立即答复手机，
/// 主线程在 [`Self::take_pending`] 时取走。服务停掉后发送端消失，泵线程自己退出。
pub struct RemoteInput {
    /// 跑着的服务；`None` 表示功能关着或没起来。
    server: Option<Server>,

    /// 待插入的文本与收到的时间；泵线程写，主线程取。
    pending: Arc<Mutex<Option<Pending>>>,

    /// 已经起起来的服务的配置；与配置里的值不同就重启服务。
    applied: Option<RemoteConfig>,

    /// 起不来时的原因，显示在菜单与面板上。
    failed: Option<String>,

    /// 配对面板，第一次打开时才建。
    panel: Option<RemotePanel>,

    /// 主线程定时器与它的 target：服务开着时在跑，服务停了就作废。
    ticker: Option<(Retained<NSTimer>, Retained<Ticker>)>,

    /// 当前 IMK 会话的 client（也就是「电脑端现在的输入框」）。会话结束就清掉。
    client: Option<Retained<AnyObject>>,
}

/// 待插入的一条推送。
struct Pending {
    text: String,
    at: Instant,
}

impl RemoteInput {
    /// 按配置决定要不要起服务。必须在主线程、IMKServer 之前调用。
    ///
    /// 令牌由 [`crate::host`] 在启动时补齐（配对凭据不进日志，也不由这里生成）。
    pub fn new(mtm: MainThreadMarker, config: &RemoteConfig) -> Self {
        let mut input = Self {
            server: None,
            pending: Arc::new(Mutex::new(None)),
            applied: None,
            failed: None,
            panel: None,
            ticker: None,
            client: None,
        };
        input.sync(mtm, config);
        input
    }

    /// 配置热加载与菜单开关都走这里：开关、端口或令牌变了就重启服务。
    pub fn sync(&mut self, mtm: MainThreadMarker, config: &RemoteConfig) {
        if self.applied.as_ref() == Some(config) {
            return;
        }
        self.stop();
        self.applied = Some(config.clone());
        if config.enabled {
            self.start(mtm, config);
        }
        self.refresh(mtm);
    }

    /// 打开配对面板。没开着服务也开着：面板上有开关、地址与二维码。
    pub fn show_panel(&mut self, mtm: MainThreadMarker, config: &RemoteConfig) {
        let (base, address, summary) = self.panel_inputs(config);
        let panel = self.panel.get_or_insert_with(|| RemotePanel::new(mtm));
        panel.show(config, &base, &address, &summary);
    }

    /// 面板要显示的三样东西：不含令牌的地址、二维码用的完整地址、状态行。
    fn panel_inputs(&self, config: &RemoteConfig) -> (String, String, String) {
        let base = self
            .base_url()
            .unwrap_or_else(|| format!("http://<本机 IP>:{}/", config.port));
        // 取不到局域网 IP 时二维码画不出来，地址那行仍然可以复制
        let address = self.address().unwrap_or_else(|| base.clone());
        (base, address, self.summary())
    }

    /// 菜单与面板上显示的一行状态。**不带令牌**：这行会进日志（debug 级），令牌不该跟着进去。
    pub fn summary(&self) -> String {
        match (&self.server, &self.failed) {
            (Some(server), _) => match self.base_url() {
                Some(url) => format!("已开启 · {url}"),
                None => format!("已开启 · 端口 {} · 没连上网络", server.port()),
            },
            (None, Some(reason)) => format!("起不来：{reason}"),
            (None, None) => "已关闭".to_string(),
        }
    }

    /// `http://<ip>:<port>/`，取不到局域网地址返回 `None`。菜单与日志用这条。
    pub fn base_url(&self) -> Option<String> {
        Some(format!(
            "http://{}/",
            std::net::SocketAddr::new(primary_lan_ip()?, self.port())
        ))
    }

    /// 二维码与「复制地址」用的那条链接（`http://<ip>:<port>/?k=<令牌>`）。取不到局域网地址返回 `None`。
    pub fn address(&self) -> Option<String> {
        Some(qingjian_remote_web::pair_url(
            primary_lan_ip()?,
            self.port(),
            &self.token(),
        ))
    }

    /// 配对令牌。没起服务时从配置里取，面板上的「复制令牌」用它。
    pub fn token(&self) -> String {
        self.applied
            .as_ref()
            .map_or(String::new(), |config| config.token.clone())
    }

    /// 面板上「复制地址」「复制令牌」两个按钮的动作。
    pub fn panel_action(&mut self, action: PanelAction) {
        match action {
            PanelAction::CopyAddress => {
                if let Some(address) = self.address() {
                    crate::host::copy_to_pasteboard(&address);
                }
            }
            PanelAction::CopyToken => {
                let token = self.token();
                if !token.is_empty() {
                    crate::host::copy_to_pasteboard(&token);
                }
            }
            // 开关由 Host 那边接手：要先写配置再热加载
            PanelAction::Toggle => {}
        }
    }

    /// 记住当前会话的 client。IMK 在 `activateServer:` 与按键事件里给的就是它。
    pub fn set_client(&mut self, object: &AnyObject) {
        self.client = Some(object.retain());
    }

    /// 会话结束：输入框没了。
    pub fn clear_client(&mut self) {
        self.client = None;
    }

    /// 当前会话的 client，没有会话（没点在任何输入框上）时为 `None`。
    pub fn client(&self) -> Option<Retained<AnyObject>> {
        self.client.clone()
    }

    /// 攒着的文本（还没插、也没过期）。插成功之后才 [`Self::clear_pending`]。
    ///
    /// 插不插由 [`crate::host::Host`] 决定：它要先看有没有 client、有没有在安全输入里。
    pub fn pending(&self) -> Option<String> {
        let Ok(mut slot) = self.pending.lock() else {
            return None;
        };
        let pending = slot.as_ref()?;
        if pending.at.elapsed() >= PENDING_TTL {
            tracing::warn!("等了太久还没等到输入框，这条手机推送丢弃");
            slot.take();
            return None;
        }
        Some(pending.text.clone())
    }

    /// 丢掉攒着的文本（插过了、或者发现没有能插的地方）。
    pub fn clear_pending(&self) {
        if let Ok(mut slot) = self.pending.lock() {
            slot.take();
        }
    }

    fn port(&self) -> u16 {
        self.server
            .as_ref()
            .map_or(DEFAULT_REMOTE_PORT, Server::port)
    }

    fn start(&mut self, mtm: MainThreadMarker, config: &RemoteConfig) {
        let (sender, inbox) = mpsc::channel::<Incoming>();
        let service = ServiceConfig {
            port: config.port,
            token: config.token.clone(),
            ..ServiceConfig::default()
        };
        match Server::start(service, sender) {
            Ok(server) => {
                tracing::info!(port = config.port, "手机输入服务已开");
                self.pump(inbox);
                self.server = Some(server);
                self.failed = None;
                self.start_timer(mtm);
            }
            Err(error) => {
                tracing::warn!(%error, port = config.port, "手机输入服务起不来");
                self.failed = Some(error.to_string());
            }
        }
    }

    /// 主线程定时器：推送到达时用户不一定已经点在某个输入框上，这时要有个主动醒来的机会。
    fn start_timer(&mut self, mtm: MainThreadMarker) {
        if self.ticker.is_some() {
            return;
        }
        let allocated = mtm.alloc::<Ticker>().set_ivars(());
        // SAFETY: NSObject 的 init，直接返回自身
        let target: Retained<Ticker> = unsafe { msg_send![super(allocated), init] };
        // SAFETY: target 与选择器都在上面定义；定时器由 run loop 持有，这里也留一份
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                TICK_INTERVAL,
                &target,
                sel!(tickRemoteInput:),
                None,
                true,
            )
        };
        // 默认模式在菜单跟踪、拖窗口时不跑；加进公共模式，菜单开着也能插
        // SAFETY: 定时器是刚建的，主 run loop 一直活着
        unsafe {
            let modes = NSRunLoopCommonModes;
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, modes);
        }
        self.ticker = Some((timer, target));
    }

    fn stop_timer(&mut self) {
        if let Some((timer, _)) = self.ticker.take() {
            timer.invalidate();
        }
    }

    /// 起一条泵线程：把服务交来的文本挪进待插入队列，并立即答复手机。
    ///
    /// 它不碰 objc，也不问系统安全输入状态（那是主线程的事，插之前还会再看一次）。
    /// 服务停掉后发送端消失，`recv` 返回错误，线程自己退出。
    fn pump(&self, inbox: mpsc::Receiver<Incoming>) {
        let pending = Arc::clone(&self.pending);
        let spawned = std::thread::Builder::new()
            .name("qingjian-remote-pump".to_string())
            .spawn(move || {
                while let Ok(incoming) = inbox.recv() {
                    let text = incoming.text().to_string();
                    tracing::info!(chars = text.chars().count(), "手机推送已收下");
                    if let Ok(mut slot) = pending.lock() {
                        *slot = Some(Pending {
                            text,
                            at: Instant::now(),
                        });
                    }
                    incoming.complete(Outcome::Queued);
                }
                tracing::debug!("手机输入的泵线程退出");
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "手机输入的泵线程起不来");
        }
    }

    fn stop(&mut self) {
        if self.server.take().is_some() {
            tracing::info!("手机输入服务已关");
        }
        self.stop_timer();
        self.clear_client();
        if let Ok(mut slot) = self.pending.lock() {
            slot.take();
        }
        self.failed = None;
    }

    /// 面板开着时把状态刷回去（服务刚开 / 关、刚起不来）。
    fn refresh(&mut self, _mtm: MainThreadMarker) {
        let Some(panel) = &self.panel else {
            return;
        };
        let config = self.applied.clone().unwrap_or_default();
        let (base, address, summary) = self.panel_inputs(&config);
        panel.refresh(&config, &base, &address, &summary);
    }
}

/// 面板上三个按钮各自的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelAction {
    /// 开关服务（写配置，由 [`crate::host::Host`] 那边接手）。
    Toggle,
    /// 把地址复制到剪贴板。
    CopyAddress,
    /// 把令牌复制到剪贴板。
    CopyToken,
}

impl PanelAction {
    /// 从按钮的 tag 认回来。
    pub fn from_tag(tag: isize) -> Option<Self> {
        Some(match tag {
            1 => Self::Toggle,
            2 => Self::CopyAddress,
            3 => Self::CopyToken,
            _ => return None,
        })
    }
}

/// 面板按钮的 target：只认 tag，动作交给 [`RemoteInput`]。
pub(crate) fn panel_action_from_sender(
    sender: Option<&objc2::runtime::AnyObject>,
) -> Option<PanelAction> {
    let button = sender?.downcast_ref::<objc2_app_kit::NSButton>()?;
    PanelAction::from_tag(button.tag())
}
