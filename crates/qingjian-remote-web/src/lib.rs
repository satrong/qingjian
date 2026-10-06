//! 手机推送上屏：输入法自己开一个局域网 Web 服务，手机浏览器打开一页表单，
//! 提交文本，壳把它插进电脑端当前的输入框。
//!
//! 服务只认三个路由（见 [`Server`]）：`GET /` 是页面，`POST /commit` 收文本，`GET /status` 报可用性。
//! 页面本身不含麦克风代码——语音由手机自己的输入法识别后打进表单，所以不需要 HTTPS、不需要麦克风权限。
//!
//! 分工：这个 crate 只管 HTTP 与鉴权，**不碰任何平台 API**。收到文本后它把一次
//! [`Incoming`] 交给壳给的通道并等回答（见 [`Outcome`]），什么时候插、插进哪，由壳决定；
//! 三端都答不上来（没有可输入的窗口、处于安全输入）时这里回 `409`，页面把原因显示出来。
//!
//! 传输是明文 HTTP，凭据只有一枚 128 位令牌，见 [`Config::token`] 与 [`Config::generate_token`]。
//! 同一局域网内谁都能连上这个端口，令牌是唯一的边界，所以默认不开（由壳决定何时 [`Server::start`]）。
//! 跨源网页推不了文本：`POST` 要求自定义头，会触发浏览器预检，而这里永不返回 CORS 头。

mod config;
mod error;
mod http;
mod net;
mod page;
mod server;
mod text;

pub use config::{Config, DEFAULT_PORT, MAX_TEXT_BYTES};
pub use error::{Error, Outcome};
pub use net::{pair_url, primary_lan_ip};
pub use server::{Incoming, Server};
pub use text::sanitize;
