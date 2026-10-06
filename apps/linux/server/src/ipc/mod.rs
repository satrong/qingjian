//! Unix socket 服务：连接内会话编号隔离、握手校验、断线回收和私有权限。
use crate::dispatch::Router;
use qingjian_platform::protocol::{ClientMessage, ServerMessage};
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
mod connection;
mod session;
use connection::serve_connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::thread;
use std::time::Instant;

/// 工人队列的活；类型本体在 [`crate::dispatch`]（socket 只是它的一个投递方）。
pub use crate::dispatch::Work;
static STOP: AtomicBool = AtomicBool::new(false);
const MAX_CLIENT_CONNECTIONS: usize = 64;
static CONNECTIONS: AtomicUsize = AtomicUsize::new(0);
/// 主线程在下一次空闲节拍退出，落盘由 Router::drop 完成。
pub fn request_shutdown() {
    STOP.store(true, Ordering::Relaxed);
}
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// 取得 Linux socket 路径；没有运行时目录时使用按用户隔离的私有临时目录。
pub fn socket_path() -> PathBuf {
    std::env::var_os("QINGJIAN_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_RUNTIME_DIR")
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(format!("/tmp/qingjian-{}", unsafe { libc::geteuid() }))
                })
                .join("qingjian.sock")
        })
}

/// 仅移除同用户、确认拒绝连接的陈旧 socket；拒绝符号链接和不安全的父目录。
pub fn bind_socket(path: &Path) -> io::Result<UnixListener> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("socket requires an absolute parent directory"))?;
    if !path.is_absolute() {
        return Err(io::Error::other("socket path must be absolute"));
    }
    if !parent.exists() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let owner = unsafe { libc::geteuid() };
    let metadata = std::fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.uid() != owner || metadata.mode() & 0o022 != 0 {
        return Err(io::Error::other(
            "socket directory must be owned by this user and not writable by others",
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.file_type().is_socket() || meta.uid() != owner {
                return Err(io::Error::other("unsafe socket path"));
            }
            match UnixStream::connect(path) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "server already running",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    std::fs::remove_file(path)?
                }
                Err(error) => return Err(error),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

pub fn serve_socket(
    path: impl AsRef<Path>,
    router: &mut Router,
    config: &qingjian_platform::Config,
) -> io::Result<()> {
    let listener = bind_socket(path.as_ref())?;
    let settings = router.display_settings();
    let (sender, receiver) = mpsc::sync_channel::<Work>(128);
    // 手机推送的泵线程要靠这个通道把文本投进来；起服务放在这儿（配置读一次，没有热加载）
    router.set_work_sender(sender.clone());
    router.sync_remote(&config.remote);
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    if CONNECTIONS.fetch_add(1, Ordering::Relaxed) >= MAX_CLIENT_CONNECTIONS {
                        CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
                        continue;
                    }
                    let sender = sender.clone();
                    let settings = settings.clone();
                    thread::spawn(move || {
                        serve_connection(stream, sender, settings);
                        CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Err(error) => {
                    tracing::error!(%error, "接受客户端连接失败");
                    break;
                }
            }
        }
    });
    // 按 Router 的节拍来 tick：在等本地整句模型就几十毫秒一次，否则一秒看一次要不要落盘学习。
    // 到点时间是绝对的，不随消息重新计时——组句期间插件每 80 毫秒问一次，若每收一条消息就重等，tick 永远到不了。
    let mut due = Instant::now() + router.next_tick();
    while !STOP.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= due {
            router.tick();
            due = Instant::now() + router.next_tick();
            continue;
        }
        match receiver.recv_timeout(due - now) {
            Ok(Work::Request(message, reply)) => {
                let _ = reply.send(router.handle_linux(message));
                // 处理完消息节拍可能变短了（按键起了防抖）：到点时间只提前不推后
                due = due.min(Instant::now() + router.next_tick());
            }
            // 手机推送：挑好目标会话、立即答复手机，插入等插件下一个事件捎带
            Ok(Work::Remote(incoming)) => router.accept_remote(incoming),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
    std::fs::remove_file(path.as_ref())?;
    Ok(())
}

fn dispatch(sender: &SyncSender<Work>, message: ClientMessage) -> Option<ServerMessage> {
    let (reply, receiver) = mpsc::channel();
    sender
        .send(Work::Request(serde_json::to_value(message).ok()?, reply))
        .ok()?;
    serde_json::from_value(receiver.recv().ok().flatten()?).ok()
}

fn dispatch_json(
    sender: &SyncSender<Work>,
    message: serde_json::Value,
) -> Option<serde_json::Value> {
    let (reply, receiver) = mpsc::channel();
    sender.send(Work::Request(message, reply)).ok()?;
    receiver.recv().ok().flatten()
}

#[cfg(test)]
mod tests;
