use serde::{Deserialize, Serialize};

/// 缺省端口与 `qingjian-remote-web::DEFAULT_PORT` 是同一个值（两边共用一个常量，改一处就够）。
pub const DEFAULT_REMOTE_PORT: u16 = qingjian_remote_web::DEFAULT_PORT;

/// `[remote]` 分节：手机推送上屏（手机浏览器打开一页表单，提交的文本插进当前输入框）。
///
/// 缺省关。令牌是配对凭据：壳在用户第一次开启时生成并写回这里，二维码与地址里都带着它。
/// 设计见 `docs/design/remote-input.md`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteConfig {
    /// 是否开这个功能。缺省关。
    pub enabled: bool,

    /// 监听端口。手机侧靠「IP + 端口」记住地址，改了要重新扫码。
    pub port: u16,

    /// 配对令牌，128 位随机（十六进制 32 字符）。空表示还没生成。
    pub token: String,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_REMOTE_PORT,
            token: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_off_on_the_shared_port() {
        let config = RemoteConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.port, DEFAULT_REMOTE_PORT);
        assert!(config.token.is_empty());
    }

    #[test]
    fn round_trips_through_toml() {
        let config = RemoteConfig {
            enabled: true,
            port: 23456,
            token: "abc".to_string(),
        };
        let text = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<RemoteConfig>(&text).unwrap(), config);
    }

    #[test]
    fn an_empty_section_falls_back_to_defaults() {
        let config: RemoteConfig = toml::from_str("").unwrap();
        assert_eq!(config, RemoteConfig::default());
    }
}
