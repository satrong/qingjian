//! 地址怎么来：手机侧只需要「IP + 端口」，所以服务要能说出自己在这台机器上是哪个 IP。
//!
//! 枚举网卡要走平台 API（macOS `getifaddrs`、Windows `GetAdaptersAddresses`），
//! 那是壳的事，这里只给一个不依赖平台的兜底：UDP `connect` 到一个不可路由的保留地址，
//! 内核会照默认路由挑出源地址，不发任何包。有线 + Wi-Fi + VPN 同时在时它只给一个，
//! 菜单里再让用户手动改也够用。

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// TEST-NET-1（RFC 5737），拿来让内核选路用，任何环境都不该有路由真的通向它。
const PROBE: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 9);

/// 这台机器在局域网里的地址（取默认路由那一份）。断网或只有回环时返回 `None`。
pub fn primary_lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(PROBE).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    is_lan(&ip).then_some(ip)
}

/// 算不算一个手机能连上的局域网地址：排除回环、0.0.0.0、169.254 链路本地（没配好地址时会拿到）。
pub fn is_lan(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !v4.is_loopback() && !v4.is_unspecified() && !v4.is_link_local() && !v4.is_broadcast()
        }
        // v6 全局地址与 ULA 都可以；链路本地要靠 scope id 才能连，不在这里拼。
        IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unspecified() && !v6.is_unicast_link_local(),
    }
}

/// 二维码与「复制地址」用的完整地址：首页带上令牌，页面读走后就把它从地址栏抹掉。
pub fn pair_url(ip: IpAddr, port: u16, token: &str) -> String {
    format!("http://{}/?k={}", SocketAddr::new(ip, port), token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_and_link_local_are_not_lan() {
        assert!(!is_lan(&IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!is_lan(&IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
        assert!(!is_lan(&IpAddr::V4(Ipv4Addr::new(169, 254, 1, 2))));
        assert!(!is_lan(&IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
    }

    #[test]
    fn private_ranges_are_lan() {
        assert!(is_lan(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 23))));
        assert!(is_lan(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))));
        assert!(is_lan(&IpAddr::V4(Ipv4Addr::new(172, 16, 3, 4))));
    }

    #[test]
    fn pair_url_carries_host_port_and_token() {
        let url = pair_url(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 23)),
            23333,
            "deadbeef",
        );
        assert_eq!(url, "http://192.168.1.23:23333/?k=deadbeef");
    }

    #[test]
    fn pair_url_brackets_v6_hosts() {
        let url = pair_url(IpAddr::V6("fd00::1".parse().unwrap()), 23333, "t");
        assert_eq!(url, "http://[fd00::1]:23333/?k=t");
    }

    #[test]
    fn probe_does_not_panic_offline() {
        // 断网时只会是 None，不该 panic 或阻塞。
        let _ = primary_lan_ip();
    }
}
