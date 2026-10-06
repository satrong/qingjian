//! 地址怎么来：手机侧只需要「IP + 端口」，所以服务要能说出自己在这台机器上是哪个 IP。
//!
//! 一台机器同时接着的东西比人以为的多：Wi-Fi、有线、访客网络、AirDrop 用的 `awdl0`、雷雳网桥、
//! Docker 的 `veth`，还有 VPN 的 `utun*` / `ppp*` / `tun*`。**手机只连得上和它同一段的那一个**，
//! 给错地址的表现是「扫得出来但打不开」，所以这里枚举网卡后按可信度排序，而不是随手拿一个。
//!
//! 排序依据三条，从硬到软：地址本身能不能用（排掉回环、`0.0.0.0`、`169.254` 链路本地）、
//! 网卡是不是虚拟/隧道（`utun`/`tun`/`tap`/`ppp`/`awdl`/`llw`/`bridge`/`docker`/`veth` 都不是给手机走的）、
//! 是不是私有网段（`192.168` / `10.` / `172.16-31`，路由器后面那几段）。同分时按接口名定序，
//! 保证每次问出来是同一个地址——面板上的二维码不能刷新一次变一次。
//!
//! 拿不到时 [`primary_lan_ip`] 退回 UDP `connect` 到一个不可路由的保留地址：内核会照默认路由挑出源地址，
//! 不发任何包。枚举失败、或枚举到的全被过滤掉（只连了 VPN、只连了虚拟机网卡）时走这条兜底。

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// TEST-NET-1（RFC 5737），拿来让内核选路用，任何环境都不该有路由真的通向它。
const PROBE: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 9);

/// 网卡名里这些前缀是虚拟网卡或隧道：手机连不上，就算地址看着正常。
///
/// 前缀匹配而不是全名，因为编号会变（`utun0`…`utun4`、`veth123`）。
const VIRTUAL_PREFIXES: &[&str] = &[
    "utun", // macOS/iOS 的 VPN 与各类隧道
    "tun", "tap", "ppp", "ipsec", // Linux 与部分 macOS VPN
    "wg",    // WireGuard
    "awdl",
    "llw", // Apple Wireless Direct Link：AirDrop / 隔空投送，手机不通过它上网
    "bridge", "br-", // 网桥
    "veth", "docker", "virbr", "vboxnet", "vmnet", // 虚拟机与容器
    "anpi", "ap1", // Apple 的内部直连接口
    "gif", "stf", // 隧道接口的老名字
];

/// 私有网段（RFC 1918）。路由器后面那几段，手机一定连得上。
fn is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || (v4.octets()[0] == 172 && (16..32).contains(&v4.octets()[1]))
        }
        // ULA（fc00::/7）是 IPv6 的私有段；全局地址要靠路由，手机上打不开的大概率是它，但排除链路本地已经够用。
        IpAddr::V6(v6) => v6.is_unique_local(),
    }
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

/// 网卡名是不是虚拟网卡或隧道。
fn is_virtual(name: &str) -> bool {
    VIRTUAL_PREFIXES
        .iter()
        .any(|prefix| name.len() >= prefix.len() && name.to_ascii_lowercase().starts_with(prefix))
}

/// 一张网卡上的一个地址，附带判定可信度要用的两项。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    name: String,
    ip: IpAddr,
}

/// 从枚举结果里挑出手机最可能连得上的那个地址。拿到空表返回 `None`。
///
/// 拆成纯函数是为了能单测：真实的网卡表在 CI 与别人的机器上都不一样。
fn pick(candidates: &[Candidate]) -> Option<IpAddr> {
    candidates
        .iter()
        .filter(|c| is_lan(&c.ip) && !is_virtual(&c.name))
        // 私有网段优先；同分按接口名定序，保证结果稳定
        .min_by(|a, b| {
            is_private(&b.ip)
                .cmp(&is_private(&a.ip))
                .then_with(|| a.name.cmp(&b.name))
        })
        .map(|c| c.ip)
}

/// 这台机器在局域网里的地址（Wi-Fi、有线等真实网卡上最可信的那个）。
pub fn primary_lan_ip() -> Option<IpAddr> {
    let picked = if_addrs::get_if_addrs().ok().map(|interfaces| {
        let candidates: Vec<Candidate> = interfaces
            .into_iter()
            .map(|i| Candidate {
                ip: i.ip(),
                name: i.name,
            })
            .collect();
        pick(&candidates)
    });
    // 枚举失败，或枚举到的全被过滤掉了（只连了 VPN、只连了虚拟机网卡）——退回让内核按默认路由挑
    picked.flatten().or_else(default_route_ip)
}

/// 让内核按默认路由挑出源地址。不发包。断网或只有回环时返回 `None`。
fn default_route_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(PROBE).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    is_lan(&ip).then_some(ip)
}

/// 二维码与「复制地址」用的完整地址：首页带上令牌，页面读走后就把它从地址栏抹掉。
pub fn pair_url(ip: IpAddr, port: u16, token: &str) -> String {
    format!("http://{}/?k={}", SocketAddr::new(ip, port), token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, last))
    }

    /// 造一张网卡表。`name:ip` 形式，省得写结构体。
    fn table(entries: &[(&str, IpAddr)]) -> Vec<Candidate> {
        entries
            .iter()
            .map(|(name, ip)| Candidate {
                name: (*name).to_string(),
                ip: *ip,
            })
            .collect()
    }

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
    fn picks_the_wifi_card_over_the_vpn_tunnel() {
        // 真实机器上就是这张表：VPN 在跑，Wi-Fi 也在，默认路由被 VPN 抢过
        let candidates = table(&[
            ("lo0", IpAddr::V4(Ipv4Addr::LOCALHOST)),
            ("awdl0", "fe80::1".parse().unwrap()),
            ("utun0", "10.9.9.1".parse().unwrap()),
            ("utun3", "10.9.9.9".parse().unwrap()),
            ("en0", v4(23)),
        ]);
        assert_eq!(pick(&candidates), Some(v4(23)));
    }

    #[test]
    fn picks_a_private_card_over_a_public_one() {
        // 有线拿到公网地址（少见但存在：运营商给的企业网），Wi-Fi 上是 10.x，Wi-Fi 更像手机在的那张
        let candidates = table(&[("en0", "203.0.113.7".parse().unwrap()), ("en1", v4(5))]);
        assert_eq!(pick(&candidates), Some(v4(5)));
    }

    #[test]
    fn skips_virtual_and_tunnel_interfaces() {
        for name in [
            "utun0", "tun0", "tap3", "ppp0", "wg0", "awdl0", "llw0", "bridge0", "br-abc",
            "veth123", "docker0", "virbr0", "vboxnet0", "anpi0", "ap1", "gif0", "stf0",
        ] {
            assert!(is_virtual(name), "{name} 应该被当成虚拟网卡");
        }
        // 真网卡不能被误伤
        for name in [
            "en0", "en1", "en5", "eth0", "wlan0", "wlp2s0", "eno1", "enp3s0",
        ] {
            assert!(!is_virtual(name), "{name} 是真实网卡，不该被过滤");
        }
    }

    #[test]
    fn virtual_interface_name_matching_ignores_case() {
        // Windows 上接口名可能是 "Tunnel 3" 这类带空格的大写形式
        assert!(is_virtual("UTUN2"));
        assert!(is_virtual("Awdl0"));
    }

    #[test]
    fn falls_through_when_only_virtual_cards_are_up() {
        let candidates = table(&[
            ("lo0", IpAddr::V4(Ipv4Addr::LOCALHOST)),
            ("utun0", "10.9.9.1".parse().unwrap()),
        ]);
        assert_eq!(pick(&candidates), None);
    }

    #[test]
    fn a_real_card_beats_a_tunnelled_one_even_when_the_tunnel_is_private() {
        // VPN 的 10.x 也是私有网段，光看网段挑不出来，必须先按网卡名排掉隧道
        let candidates = table(&[("utun0", "10.8.0.1".parse().unwrap()), ("en0", v4(23))]);
        assert_eq!(pick(&candidates), Some(v4(23)));
    }

    #[test]
    fn same_rank_is_broken_by_name_so_the_answer_is_stable() {
        // 两张都是私有网段：必须每次都给同一个，否则面板刷新一次二维码变一次
        let first = pick(&table(&[("en1", v4(9)), ("en0", v4(23))]));
        let second = pick(&table(&[("en0", v4(23)), ("en1", v4(9))]));
        assert_eq!(first, second);
        assert_eq!(first, Some(v4(23)));
    }

    #[test]
    fn rfc1918_172_16_to_31_is_private_but_neighbours_are_not() {
        assert!(is_private(&IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(is_private(&IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255))));
        assert!(!is_private(&IpAddr::V4(Ipv4Addr::new(172, 15, 0, 1))));
        assert!(!is_private(&IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1))));
    }

    #[test]
    fn ipv6_ula_is_private() {
        assert!(is_private(&"fd00::1".parse().unwrap()));
        assert!(!is_private(&"2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn pair_url_carries_host_port_and_token() {
        let url = pair_url(v4(23), 23333, "deadbeef");
        assert_eq!(url, "http://192.168.1.23:23333/?k=deadbeef");
    }

    #[test]
    fn pair_url_brackets_v6_hosts() {
        let url = pair_url("fd00::1".parse().unwrap(), 23333, "t");
        assert_eq!(url, "http://[fd00::1]:23333/?k=t");
    }

    #[test]
    fn primary_lan_ip_never_returns_a_dead_end() {
        // 不断网也返回 None，只要求它不 panic、不给回环
        if let Some(ip) = primary_lan_ip() {
            assert!(is_lan(&ip), "给出了 {ip}，它不是能用的局域网地址");
        }
    }

    #[test]
    fn default_route_probe_does_not_panic_offline() {
        // 断网时只会是 None，不该 panic 或阻塞。
        let _ = default_route_ip();
    }
}
