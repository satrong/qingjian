//! 「手机输入」页：电脑没有麦克风时，让手机浏览器打开一页表单，提交的文本插进当前输入框。
//!
//! Windows 这边不画二维码（系统没有生成二维码的 API，另找库不值当）：地址给成可选中的一行，
//! 用手机相机扫不了就把它复制到手机浏览器、隔空投送到手机上都行。
//!
//! 开关写回 `config.toml` 的 `[remote]`，Server 一秒内热加载起停服务（见 `docs/design/remote-input.md`）。

use qingjian_remote_web::{pair_url, primary_lan_ip};
use windows_reactor::*;

use crate::panel::controls::{field, note, page};
use crate::panel::{Message, Settings};

pub(crate) fn view(settings: &Settings, context: &mut ViewContext<Settings>) -> View {
    let remote = &settings.config.remote;
    let url = match primary_lan_ip() {
        Some(ip) => pair_url(ip, remote.port, &remote.token),
        // 取不到局域网地址：多半是没连网络或只有 VPN，文案说清楚，端口还是配置里的那个
        None => format!("http://<本机 IP>:{}/?k={}", remote.port, remote.token),
    };
    let rows = [
        note(
            "电脑没有麦克风时，手机当麦克风：手机和电脑连同一个 Wi-Fi，手机浏览器打开下面这页，\
             用手机自己的输入法说话，文本会插进电脑上正在输入的那个文本框。",
        ),
        field(
            "开启手机输入",
            "开启后输入法会在本机开一个只给局域网用的小服务（默认端口 23333）。",
            ToggleSwitch::new()
                .is_on(remote.enabled)
                .on_toggled(context.callback(Message::RemoteEnabled)),
        ),
        field(
            "手机上打开",
            "扫不了二维码就把这行复制到手机浏览器（隔空投送、微信文件传输助手都行）。",
            TextBlock::new().text(url).is_text_selection_enabled(true),
        ),
        field(
            "配对令牌",
            "相当于这一页的钥匙，只有它能往这台电脑推文本，别外传。",
            TextBlock::new()
                .text(if remote.token.is_empty() {
                    "还没生成，重启输入法后自动生成".to_string()
                } else {
                    remote.token.clone()
                })
                .is_text_selection_enabled(true),
        ),
        note(
            "文本插进的是「最近用过输入框的那个窗口」，所以推送前先在电脑上点一下要写字的地方；\
             密码框里不会插。文本会替换掉正在组的拼音（一个完整句子不该插在拼音中间）。",
        ),
    ];
    page("手机输入", StackPanel::new().spacing(16.0).children(rows))
}
