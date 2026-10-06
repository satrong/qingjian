//! 手机推送上屏在 Server 侧的落点：起停服务、挑目标会话、把文本捎给 DLL。
//!
//! 命名管道是一问一答：Server 不能主动给 DLL 推帧（非应答帧会被对端当成上一次请求的应答而错位），
//! 所以待插入的文本搭既有应答的便车——组句期间 DLL 收 `Update`（`Poll` 的应答），空闲时收 `ModeSync`
//! （`SyncMode` 的应答，320 ms 一次）。字段是 `#[serde(default)]` 的可选值，老 DLL 读不到就忽略，不用升协议版本。
//!
//! 目标会话：优先 `focused`（最近收过键的会话），没有就用最近开过会话的那个（用户刚点进输入框还没打字的情况）；
//! 私密输入框（密码）不作为目标。挑好后先把它的组句清掉，手机来的是完整一句话，不该和正在打的拼音混在一起。
//!
//! 手机那边的答复不等插入：HTTP 线程把文本交给泵线程（`crate::remote`），泵线程立刻回 `Queued`。

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use qingjian_platform::RemoteConfig;
use qingjian_platform::protocol::SessionId;
use qingjian_remote_web::{Config as ServiceConfig, Incoming, Outcome, Server};

use super::Router;
use crate::ipc::Work;

/// 待插入文本能等多久。等不到就丢：宁可不插，也不要过很久以后插到不相干的地方。
const PENDING_TTL: Duration = Duration::from_secs(30);

/// Server 侧的这份服务。全部方法都在 Router 所在的工人线程上调用。
#[derive(Default)]
pub(crate) struct Remote {
    /// 跑着的服务；`None` 表示功能关着或没起来。
    server: Option<Server>,

    /// 已经起起来的服务的配置；与配置里的值不同就重启。
    applied: Option<RemoteConfig>,

    /// 起不来时的原因，写进日志。
    failed: Option<String>,

    /// 待插入的文本与目标会话；插过或过期就没了。
    pending: Option<Pending>,
}

/// 待插入的一条推送。
struct Pending {
    text: String,
    target: SessionId,
    at: Instant,
}

impl Router {
    /// 按配置起停服务。启动与每次配置热加载都走这里（对应 macOS 的 `apply_config`）。
    ///
    /// `pub` 是为了集成测试能直接起停；本 crate 在非 Windows 上也会编译（跑 Engine 侧测试）。
    pub fn sync_remote(&mut self, config: &RemoteConfig) {
        if self.remote.applied.as_ref() == Some(config) {
            return;
        }
        self.stop_remote();
        self.remote.applied = Some(config.clone());
        if !config.enabled {
            return;
        }
        let Some(work) = self.work.clone() else {
            tracing::warn!("没有工人通道，手机输入服务不启动");
            return;
        };
        let (sink, inbox) = std::sync::mpsc::channel::<Incoming>();
        let service = ServiceConfig {
            port: config.port,
            token: config.token.clone(),
            ..ServiceConfig::default()
        };
        match Server::start(service, sink) {
            Ok(server) => {
                tracing::info!(port = config.port, "手机输入服务已开");
                self.remote.server = Some(server);
                self.remote.failed = None;
                spawn_pump(inbox, work);
            }
            Err(error) => {
                tracing::warn!(%error, port = config.port, "手机输入服务起不来");
                self.remote.failed = Some(error.to_string());
            }
        }
    }

    /// 收下一条手机推送：挑好目标会话、清掉它的组句、存起来等捎给 DLL，并立刻答复手机。
    ///
    /// 由命名管道的工人循环调用（[`Work::Remote`]），所以是 `pub`：本 crate 在非 Windows 上
    /// 也会编译（跑 Engine 侧测试），那里没有管道服务，私有方法会被判成死代码。
    pub fn accept_remote(&mut self, incoming: Incoming) {
        let Some(target) = self.remote_target() else {
            tracing::info!("手机推送到达时没有可插入的输入框");
            incoming.complete(Outcome::NoTarget);
            return;
        };
        self.remote.pending = Some(Pending {
            text: incoming.text().to_string(),
            target,
            at: Instant::now(),
        });
        // 组句里的拼音先丢掉：手机来的是完整一句话，不该插在拼音中间
        if self.focused == Some(target) {
            self.reset_composition();
        }
        tracing::info!(
            chars = self.remote.pending.as_ref().map(|p| p.text.chars().count()),
            ?target,
            "手机推送已收下"
        );
        incoming.complete(Outcome::Queued);
    }

    /// 服务实际监听的端口；没开服务为 `None`。配置里写 0 时这里才是真拿到的那个。
    pub fn remote_port(&self) -> Option<u16> {
        self.remote.server.as_ref().map(Server::port)
    }

    /// 组这个会话的回帧时取走待插入的文本（取走即清，一个会话只插一次）。
    pub(crate) fn take_remote_for(&mut self, session: SessionId) -> Option<String> {
        let pending = self.remote.pending.as_ref()?;
        if pending.target != session {
            return None;
        }
        if pending.at.elapsed() >= PENDING_TTL {
            tracing::warn!("等了太久这个会话也没来取，丢弃这条手机推送");
            self.remote.pending = None;
            return None;
        }
        self.remote.pending.take().map(|pending| pending.text)
    }

    fn stop_remote(&mut self) {
        if self.remote.server.take().is_some() {
            tracing::info!("手机输入服务已关");
        }
        self.remote.pending = None;
    }

    /// 目标会话关掉了：等着它的文本没有收件人了，丢掉。
    pub(crate) fn drop_remote_for(&mut self, session: SessionId) {
        if self
            .remote
            .pending
            .as_ref()
            .is_some_and(|p| p.target == session)
        {
            tracing::info!("目标会话已关闭，这条手机推送丢弃");
            self.remote.pending = None;
        }
    }

    /// 目标会话：最近收过键的那个（`focused`），没有就退到最近开过会话的；私密输入框不要。
    fn remote_target(&self) -> Option<SessionId> {
        let usable = |session: SessionId| {
            self.sessions
                .get(&session)
                .is_some_and(|info| !info.private)
        };
        self.focused
            .filter(|session| usable(*session))
            .or_else(|| self.last_session())
            .filter(|session| usable(*session))
    }

    /// 最近开过会话的那个（`OpenSession` 会记；`focused` 只在收键时才动，光点进输入框不算）。
    fn last_session(&self) -> Option<SessionId> {
        self.last_session
    }
}

/// 泵线程：服务线程把文本交过来，这里转成 [`Work`] 投给工人线程（Router 只在工人线程上跑）。
/// 服务停掉后发送端消失，`recv` 返回错误，线程自己退出。
fn spawn_pump(inbox: std::sync::mpsc::Receiver<Incoming>, work: Sender<Work>) {
    let spawned = std::thread::Builder::new()
        .name("qingjian-remote-pump".to_string())
        .spawn(move || {
            while let Ok(incoming) = inbox.recv() {
                if work.send(Work::Remote(incoming)).is_err() {
                    break;
                }
            }
            tracing::debug!("手机输入的泵线程退出");
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "手机输入的泵线程起不来");
    }
}
