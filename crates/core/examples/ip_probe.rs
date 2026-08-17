//! 三段 IP 查询探针：不启动 UI，独立验证本机路由与 WinHTTP 代理语义。

use wisp_core::{IpKind, IpService};

fn main() {
    for (label, kind) in [
        ("内网", IpKind::Local),
        ("直连公网", IpKind::Direct),
        ("系统代理", IpKind::Proxy),
    ] {
        println!("{label}: {:?}", IpService::lookup(kind));
    }
}
