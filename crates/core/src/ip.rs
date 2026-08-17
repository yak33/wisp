//! IP 查询服务：主内网地址、直连公网出口与系统代理出口。
//!
//! 公网查询使用 Windows 自带 WinHTTP。直连与代理会话显式采用不同访问类型，
//! 避免“请求两次同一客户端”导致代理开启后两项仍无法区分。调用是同步的，
//! 壳层必须在后台线程执行。

use std::{
    ffi::c_void,
    net::{IpAddr, Ipv4Addr, UdpSocket},
    ptr,
};

use anyhow::{Context as _, Result, anyhow, bail};
use windows::{
    Win32::Networking::WinHttp::{
        INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
        WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen,
        WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpSetTimeouts,
    },
    core::{Error as WindowsError, PCWSTR, w},
};

const REQUEST_TIMEOUT_MS: i32 = 4_000;
const MAX_RESPONSE_BYTES: usize = 512;

const DIRECT_IP_ENDPOINTS: &[IpEndpoint] = &[
    IpEndpoint::plain("IPW", "4.ipw.cn", "/"),
    IpEndpoint::ipip("IPIP", "myip.ipip.net", "/"),
];
const PROXY_IP_ENDPOINTS: &[IpEndpoint] = &[
    IpEndpoint::plain("ipify", "api4.ipify.org", "/"),
    IpEndpoint::plain("AWS", "checkip.amazonaws.com", "/"),
];

#[derive(Debug, Clone, Copy)]
struct IpEndpoint {
    name: &'static str,
    host: &'static str,
    path: &'static str,
    response_format: IpResponseFormat,
}

impl IpEndpoint {
    const fn plain(name: &'static str, host: &'static str, path: &'static str) -> Self {
        Self {
            name,
            host,
            path,
            response_format: IpResponseFormat::Plain,
        }
    }

    const fn ipip(name: &'static str, host: &'static str, path: &'static str) -> Self {
        Self {
            name,
            host,
            path,
            response_format: IpResponseFormat::IpipText,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum IpResponseFormat {
    Plain,
    IpipText,
}

/// IP 页面中的固定查询维度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpKind {
    Local,
    Direct,
    Proxy,
}

/// 单项查询结果。错误作为稳定文本跨线程交给壳层展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpLookup {
    Available(IpAddr),
    Unavailable(String),
}

impl IpLookup {
    fn from_result(result: Result<IpAddr>) -> Self {
        match result {
            Ok(address) => Self::Available(address),
            Err(error) => Self::Unavailable(format!("{error:#}")),
        }
    }
}

/// 内置 IP 查询入口。当前只有一套确定实现，不制造可替换接口。
pub struct IpService;

impl IpService {
    /// 查询单一维度。三个维度互不依赖，壳层可并行执行并逐项呈现。
    pub fn lookup(kind: IpKind) -> IpLookup {
        let result = match kind {
            IpKind::Local => primary_local_ipv4(),
            IpKind::Direct => public_ip(WINHTTP_ACCESS_TYPE_NO_PROXY, DIRECT_IP_ENDPOINTS),
            IpKind::Proxy => public_ip(WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PROXY_IP_ENDPOINTS),
        };
        IpLookup::from_result(result)
    }
}

/// UDP connect 只进行系统路由选择，不发送数据；由此得到访问外网时实际使用的
/// 主 IPv4，避免枚举网卡时把虚拟网卡、断开的 Wi-Fi 一并展示给用户。
fn primary_local_ipv4() -> Result<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).context("无法读取本机网络")?;
    socket
        .connect((Ipv4Addr::new(1, 1, 1, 1), 80))
        .context("当前没有可用网络")?;
    let address = socket.local_addr().context("无法确定内网地址")?.ip();
    match address {
        IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Ok(address),
        _ => bail!("未找到可用的内网 IPv4"),
    }
}

fn public_ip(access_type: WINHTTP_ACCESS_TYPE, endpoints: &[IpEndpoint]) -> Result<IpAddr> {
    first_available_ip(endpoints, |endpoint| {
        request_public_ip(access_type, endpoint)
    })
}

fn first_available_ip(
    endpoints: &[IpEndpoint],
    mut request: impl FnMut(&IpEndpoint) -> Result<IpAddr>,
) -> Result<IpAddr> {
    let mut failures = Vec::with_capacity(endpoints.len());
    for endpoint in endpoints {
        match request(endpoint) {
            Ok(address) => return Ok(address),
            Err(error) => failures.push(format!("{}: {error:#}", endpoint.name)),
        }
    }

    bail!("所有公网 IP 服务均不可用（{}）", failures.join("；"))
}

fn request_public_ip(access_type: WINHTTP_ACCESS_TYPE, endpoint: &IpEndpoint) -> Result<IpAddr> {
    let host = null_terminated_utf16(endpoint.host);
    let path = null_terminated_utf16(endpoint.path);

    // SAFETY: 每个非空句柄立即进入 InternetHandle，所有退出路径均由 Drop 关闭；
    // WinHTTP 使用同步模式，UTF-16 缓冲区在调用完成前始终有效。
    unsafe {
        let session = InternetHandle::new(
            WinHttpOpen(
                w!("Wisp/0.1"),
                access_type,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ),
            "初始化网络请求失败",
        )?;
        WinHttpSetTimeouts(
            session.raw(),
            REQUEST_TIMEOUT_MS,
            REQUEST_TIMEOUT_MS,
            REQUEST_TIMEOUT_MS,
            REQUEST_TIMEOUT_MS,
        )
        .context("设置网络超时失败")?;

        let connection = InternetHandle::new(
            WinHttpConnect(
                session.raw(),
                PCWSTR(host.as_ptr()),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            ),
            "连接 IP 服务失败",
        )?;
        let request = InternetHandle::new(
            WinHttpOpenRequest(
                connection.raw(),
                w!("GET"),
                PCWSTR(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                ptr::null(),
                WINHTTP_FLAG_SECURE,
            ),
            "创建 IP 请求失败",
        )?;

        WinHttpSendRequest(request.raw(), None, None, 0, 0, 0).context("发送 IP 请求失败")?;
        WinHttpReceiveResponse(request.raw(), ptr::null_mut()).context("接收 IP 响应失败")?;

        ensure_success_status(request.raw())?;
        parse_ip_response(
            &read_small_response(request.raw())?,
            endpoint.response_format,
        )
    }
}

fn null_terminated_utf16(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

unsafe fn ensure_success_status(request: *mut c_void) -> Result<()> {
    let mut status = 0_u32;
    let mut size = size_of::<u32>() as u32;
    let mut index = 0_u32;
    unsafe {
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(ptr::from_mut(&mut status).cast()),
            &mut size,
            &mut index,
        )
    }
    .context("读取 IP 服务状态失败")?;

    if !(200..300).contains(&status) {
        bail!("IP 服务返回 HTTP {status}");
    }
    Ok(())
}

unsafe fn read_small_response(request: *mut c_void) -> Result<Vec<u8>> {
    let mut response = Vec::with_capacity(64);
    loop {
        let mut buffer = [0_u8; 64];
        let mut read = 0_u32;
        unsafe {
            WinHttpReadData(
                request,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut read,
            )
        }
        .context("读取 IP 响应失败")?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..read as usize]);
        if response.len() > MAX_RESPONSE_BYTES {
            bail!("IP 服务响应异常");
        }
    }
    Ok(response)
}

fn parse_ip_response(response: &[u8], format: IpResponseFormat) -> Result<IpAddr> {
    let text = std::str::from_utf8(response)
        .context("IP 服务响应不是 UTF-8")?
        .trim();
    let address = match format {
        IpResponseFormat::Plain => text,
        IpResponseFormat::IpipText => text
            .strip_prefix("当前 IP：")
            .or_else(|| text.strip_prefix("当前 IP:"))
            .and_then(|content| content.split_whitespace().next())
            .ok_or_else(|| anyhow!("IPIP 返回了未知格式"))?,
    };
    address
        .parse()
        .map_err(|_| anyhow!("IP 服务返回了无效地址"))
}

struct InternetHandle(*mut c_void);

impl InternetHandle {
    fn new(raw: *mut c_void, message: &'static str) -> Result<Self> {
        if raw.is_null() {
            Err(anyhow!("{message}: {}", WindowsError::from_win32()))
        } else {
            Ok(Self(raw))
        }
    }

    fn raw(&self) -> *mut c_void {
        self.0
    }
}

impl Drop for InternetHandle {
    fn drop(&mut self) {
        // SAFETY: 构造时已保证非空，且句柄所有权不复制、不外泄。
        unsafe {
            _ = WinHttpCloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_response_accepts_ipv4_ipv6_and_surrounding_whitespace() {
        let cases = [
            (&b"203.0.113.7"[..], "203.0.113.7"),
            (&b"  2001:db8::8\r\n"[..], "2001:db8::8"),
        ];

        for (input, expected) in cases {
            assert_eq!(
                parse_ip_response(input, IpResponseFormat::Plain)
                    .unwrap()
                    .to_string(),
                expected
            );
        }
    }

    #[test]
    fn ip_response_accepts_the_documented_ipip_text_format() {
        let response = "当前 IP：203.0.113.7  来自于：中国 山东 济南".as_bytes();

        assert_eq!(
            parse_ip_response(response, IpResponseFormat::IpipText)
                .unwrap()
                .to_string(),
            "203.0.113.7"
        );
    }

    #[test]
    fn ip_response_rejects_unknown_or_structured_content() {
        assert!(parse_ip_response(br#"{"ip":"203.0.113.7"}"#, IpResponseFormat::Plain).is_err());
        assert!(parse_ip_response(b"not-an-ip", IpResponseFormat::Plain).is_err());
        assert!(parse_ip_response(b"203.0.113.7", IpResponseFormat::IpipText).is_err());
    }

    #[test]
    fn public_ip_falls_back_to_the_next_endpoint() {
        let mut attempts = Vec::new();
        let address = first_available_ip(DIRECT_IP_ENDPOINTS, |endpoint| {
            attempts.push(endpoint.name);
            match endpoint.name {
                "IPW" => Err(anyhow!("连接失败")),
                _ => Ok("203.0.113.7".parse().unwrap()),
            }
        })
        .unwrap();

        assert_eq!(address.to_string(), "203.0.113.7");
        assert_eq!(attempts, ["IPW", "IPIP"]);
    }

    #[test]
    fn public_ip_preserves_every_endpoint_failure() {
        let error = first_available_ip(DIRECT_IP_ENDPOINTS, |endpoint| {
            Err(anyhow!("{} 不可达", endpoint.host))
        })
        .unwrap_err()
        .to_string();

        assert!(error.contains("IPW: 4.ipw.cn 不可达"));
        assert!(error.contains("IPIP: myip.ipip.net 不可达"));
    }
}
