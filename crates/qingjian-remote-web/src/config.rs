//! 服务的可调项：端口、配对令牌，以及挡住滥用与卡死的几个上限。壳构造 [`Config`] 后交给 [`Server::start`]。

use std::time::Duration;

/// 缺省端口。固定端口是刻意的：手机侧只要记住「IP + 23333」，换了端口整对配对关系就失效。
pub const DEFAULT_PORT: u16 = 23333;

/// 一次提交的上限，UTF-8 字节数。中文三字节一个字，4096 够写一千多字，远超一次语音输入的长度。
pub const MAX_TEXT_BYTES: usize = 4096;

/// 平台侧回答一次上屏的等待上限。超时回 `503`，页面提示「电脑可能已休眠」而不是一直转圈。
const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_millis(800);

/// 同时在处理的连接数上限。局域网里正常只有手机一个浏览器，超了直接回 `503`，不排队等。
const DEFAULT_MAX_CONNECTIONS: usize = 8;

/// 每分钟允许的提交次数。防的是有人拿到令牌后把上屏通道当消息队列刷。
const DEFAULT_MAX_COMMITS_PER_MINUTE: u32 = 60;

/// 服务参数。字段公开，按需覆盖；令牌以外的项一般保持缺省。
#[derive(Debug, Clone)]
pub struct Config {
    /// 监听端口，绑全部网卡（`0.0.0.0`）。`0` 让系统挑一个空闲端口，仅测试用。
    pub port: u16,

    /// 配对令牌，`POST /commit` 的 `X-Qingjian-Token` 头必须与它一致。空串是非法配置。
    pub token: String,

    /// 单次提交的字节上限，见 [`MAX_TEXT_BYTES`]。
    pub max_text_bytes: usize,

    /// 交文本给壳之后等多久算没答复。
    pub response_timeout: Duration,

    /// 同时处理的连接数上限。
    pub max_connections: usize,

    /// 每分钟提交次数上限，`0` 表示不限（仅测试用）。
    pub max_commits_per_minute: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            token: String::new(),
            max_text_bytes: MAX_TEXT_BYTES,
            response_timeout: DEFAULT_RESPONSE_TIMEOUT,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            max_commits_per_minute: DEFAULT_MAX_COMMITS_PER_MINUTE,
        }
    }
}

impl Config {
    /// 校验配置。返回错说清是哪一项，不合法时 [`crate::Server::start`] 拒绝监听。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.token.is_empty() {
            return Err("token must not be empty");
        }
        if self.max_text_bytes == 0 {
            return Err("text limit must not be 0");
        }
        Ok(())
    }

    /// 令牌与给定值是否一致。不一致花的时间与一致时相同，避免逐字节早退泄露前缀。
    pub fn token_matches(&self, presented: &str) -> bool {
        let (want, got) = (self.token.as_bytes(), presented.as_bytes());
        if want.len() != got.len() {
            return false;
        }
        want.iter()
            .zip(got)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
    }

    /// 生成一枚新令牌（122 位随机，取十六进制 32 字符）。壳在用户第一次开启服务时生成并写进配置。
    pub fn generate_token() -> String {
        uuid::Uuid::new_v4().simple().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = Config {
            token: Config::generate_token(),
            ..Config::default()
        };
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn empty_token_is_rejected() {
        assert!(Config::default().validate().is_err());
    }

    #[test]
    fn zero_text_limit_is_rejected() {
        let config = Config {
            token: "t".to_string(),
            max_text_bytes: 0,
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn token_comparison_accepts_only_the_exact_value() {
        let config = Config {
            token: "9f2c".to_string(),
            ..Config::default()
        };
        assert!(config.token_matches("9f2c"));
        assert!(!config.token_matches("9f2"));
        assert!(!config.token_matches("9f2d"));
        assert!(!config.token_matches(""));
    }

    #[test]
    fn generated_tokens_differ() {
        assert_ne!(Config::generate_token(), Config::generate_token());
        assert_eq!(Config::generate_token().len(), 32);
    }
}
