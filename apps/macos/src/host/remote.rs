//! 手机推送上屏在 Host 上的动作：开关、面板、菜单状态，以及把攒着的文本插进输入框。
//!
//! client 只有一个来源：IMK 交给我们的那个对象。会话开始（`activateServer:`）时存进
//! [`RemoteInput::set_client`]，会话结束（`deactivateServer:`）时清掉。
//! 有了它两条插入路径都能走：IMK 回调顺手用它，定时器醒来也能用它——
//! 而系统自己的 `NSTextInputManager.currentClient` 在输入法进程里恒为 nil，问不到。
//!
//! 两条路都过 [`Host::insert_remote`] 的守卫：安全输入、登录窗口都不插；
//! 组句中的拼音会先丢掉——手机来的是完整一句话，不该和正在打的拼音混在一起。

use objc2::MainThreadMarker;

use super::*;
use crate::imk::{TextClient, secure_input};
use crate::menubar::MenuAction;

/// 登录 / 锁屏窗口的 bundle identifier：那边会被 IMK 激活，但没有文本框可插。
const LOGIN_WINDOW: &str = "com.apple.loginwindow";

impl Host {
    /// 开关手机推送上屏：菜单勾选与面板上的按钮都走这里，先落盘再热加载。
    pub fn toggle_remote(&mut self) {
        let on = !self.settings.config().remote.enabled;
        if !self.settings.set_bool("remote", "enabled", on) {
            return;
        }
        let mtm = MainThreadMarker::new().expect("菜单动作在主线程");
        let config = self.settings.config().remote.clone();
        self.remote.sync(mtm, &config);
        self.sync_remote_menu();
    }

    /// 打开配对面板（二维码、地址、开关）。服务没开也能看地址，方便先复制到手机。
    pub fn show_remote_panel(&mut self) {
        let mtm = MainThreadMarker::new().expect("菜单动作在主线程");
        let config = self.settings.config().remote.clone();
        self.remote.show_panel(mtm, &config);
    }

    /// 服务状态变了：把菜单里的状态行刷新一下。
    pub fn sync_remote_menu(&mut self) {
        let enabled = self.settings.config().remote.enabled;
        self.menu.sync_remote(enabled, &self.remote.summary());
    }

    /// IMK 回调带来的 client：记住它（有会话就意味着有输入框），并把攒着的文本插进去。
    pub fn flush_remote(&mut self, client: TextClient<'_>) -> bool {
        self.remote.set_client(client.object());
        let Some(text) = self.remote.pending() else {
            return false;
        };
        self.insert_remote(text, client)
    }

    /// 会话结束：这次的输入框没了，别再往里插。
    pub fn end_remote_session(&mut self) {
        self.remote.clear_client();
    }

    /// 定时器醒来：有文本、有会话中的输入框就插进去；没有输入框就留着文本等下一次 IMK 回调。
    pub fn flush_remote_now(&mut self) {
        if self.remote.pending().is_none() {
            return;
        }
        let Some(client) = self.remote.client() else {
            return;
        };
        let Some(text) = self.remote.pending() else {
            return;
        };
        self.insert_remote(text, TextClient::new(&client));
    }

    /// 真正上屏：守卫不过就丢掉文本，返回是否插了。
    fn insert_remote(&mut self, text: String, client: TextClient<'_>) -> bool {
        // 安全输入（密码框）与登录窗口都不插：那里没有可写的输入框，也不该有
        if secure_input::enabled() || self.engine.application() == Some(LOGIN_WINDOW) {
            tracing::warn!("这台机器上没有可插入的输入框，这条手机推送丢弃");
            self.remote.clear_pending();
            return false;
        }
        self.engine.break_chain();
        self.cancel_prediction();
        self.window.hide();
        self.remote.clear_pending();
        tracing::info!(chars = text.chars().count(), "手机推送已上屏");
        client.insert_text(&text);
        true
    }
}

/// 菜单动作里与手机推送有关的两项在这里统一处理（`MenuAction` 的其余分支在 settings.rs）。
pub(crate) fn perform_remote(host: &mut Host, action: MenuAction) -> bool {
    match action {
        MenuAction::ToggleRemote => host.toggle_remote(),
        MenuAction::OpenRemotePanel => host.show_remote_panel(),
        _ => return false,
    }
    true
}
