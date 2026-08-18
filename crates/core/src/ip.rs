//! IP 查询服务：三段出口、归属地与常用站点 HTTPS 响应耗时。
//!
//! 公网查询使用 Windows 自带 WinHTTP。直连与代理会话显式采用不同访问类型，
//! 避免“请求两次同一客户端”导致代理开启后两项仍无法区分。调用是同步的，
//! 壳层必须在后台线程执行。

use std::{
    ffi::c_void,
    net::{IpAddr, Ipv4Addr, UdpSocket},
    ptr,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
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
const LATENCY_TIMEOUT_MS: i32 = 3_500;
const MAX_IP_RESPONSE_BYTES: usize = 512;
const MAX_GEO_RESPONSE_BYTES: usize = 4 * 1024;

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

/// 公网地址的简要归属信息。字段可能因上游数据库缺失而为空。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpLocation {
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub network: Option<String>,
}

impl IpLocation {
    fn from_parts(
        country: Option<String>,
        region: Option<String>,
        city: Option<String>,
        network: Option<String>,
    ) -> Result<Self> {
        let location = Self {
            country: normalized(country),
            region: normalized(region),
            city: normalized(city),
            network: normalized(network),
        };
        if [
            &location.country,
            &location.region,
            &location.city,
            &location.network,
        ]
        .into_iter()
        .all(Option::is_none)
        {
            bail!("归属地服务未返回有效字段");
        }
        Ok(location)
    }
}

/// 归属地查询独立于 IP 查询，失败时仍保留已经得到的地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpLocationLookup {
    Available(IpLocation),
    Unavailable(String),
}

impl IpLocationLookup {
    fn from_result(result: Result<IpLocation>) -> Self {
        match result {
            Ok(location) => Self::Available(location),
            Err(error) => Self::Unavailable(format!("{error:#}")),
        }
    }
}

/// IP 工具中的固定网络探测目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkSite {
    Baidu,
    NetEase,
    Aliyun,
    TencentCloud,
    GitHub,
    Google,
    YouTube,
    Amazon,
}

impl NetworkSite {
    pub const ALL: [Self; 8] = [
        Self::Baidu,
        Self::NetEase,
        Self::Aliyun,
        Self::TencentCloud,
        Self::GitHub,
        Self::Google,
        Self::YouTube,
        Self::Amazon,
    ];

    /// 站点的稳定展示名称。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Baidu => "百度",
            Self::NetEase => "网易",
            Self::Aliyun => "阿里云",
            Self::TencentCloud => "腾讯云",
            Self::GitHub => "GitHub",
            Self::Google => "Google",
            Self::YouTube => "YouTube",
            Self::Amazon => "Amazon",
        }
    }

    /// 站点所属网络区域，仅用于帮助用户快速比较境内外链路。
    pub const fn scope(self) -> &'static str {
        match self {
            Self::Baidu | Self::NetEase | Self::Aliyun | Self::TencentCloud => "境内",
            Self::GitHub | Self::Google | Self::YouTube | Self::Amazon => "境外",
        }
    }

    fn endpoint(self) -> (&'static str, &'static str) {
        match self {
            Self::Baidu => ("www.baidu.com", "/"),
            Self::NetEase => ("www.163.com", "/"),
            Self::Aliyun => ("www.aliyun.com", "/"),
            Self::TencentCloud => ("cloud.tencent.com", "/"),
            Self::GitHub => ("github.com", "/"),
            Self::Google => ("www.google.com", "/"),
            Self::YouTube => ("www.youtube.com", "/"),
            Self::Amazon => ("www.amazon.com", "/"),
        }
    }
}

/// 单站 HTTPS 可达性与响应耗时。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteLatencyLookup {
    Reachable(Duration),
    Unreachable(String),
}

impl SiteLatencyLookup {
    fn from_result(result: Result<Duration>) -> Self {
        match result {
            Ok(latency) => Self::Reachable(latency),
            Err(error) => Self::Unreachable(format!("{error:#}")),
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

    /// 查询指定公网地址的国家、地区、城市与网络提供方。
    pub fn locate(address: IpAddr) -> IpLocationLookup {
        IpLocationLookup::from_result(locate_public_ip(address))
    }

    /// 测量一次 HTTPS 请求从解析、建连、代理、TLS 到收到响应头的耗时。
    pub fn measure(site: NetworkSite) -> SiteLatencyLookup {
        SiteLatencyLookup::from_result(measure_site_latency(site))
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
    with_https_response(
        access_type,
        endpoint.host,
        endpoint.path,
        REQUEST_TIMEOUT_MS,
        |request| {
            ensure_success_status(request)?;
            parse_ip_response(
                &read_bounded_response(request, MAX_IP_RESPONSE_BYTES)?,
                endpoint.response_format,
            )
        },
    )
}

fn locate_public_ip(address: IpAddr) -> Result<IpLocation> {
    match request_ipwhois_location(address) {
        Ok(location) => Ok(location),
        Err(primary_error) => request_ip_sb_location(address).map_err(|fallback_error| {
            anyhow!("归属地服务均不可用（ipwho.is: {primary_error:#}；IP.SB: {fallback_error:#}）")
        }),
    }
}

fn request_ipwhois_location(address: IpAddr) -> Result<IpLocation> {
    let path = format!(
        "/{address}?lang=zh-CN&fields=success,message,country,region,city,connection.isp,connection.org"
    );
    with_https_response(
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
        "ipwho.is",
        &path,
        REQUEST_TIMEOUT_MS,
        |request| {
            ensure_success_status(request)?;
            parse_ipwhois_location(&read_bounded_response(request, MAX_GEO_RESPONSE_BYTES)?)
        },
    )
}

fn request_ip_sb_location(address: IpAddr) -> Result<IpLocation> {
    let path = format!("/geoip/{address}");
    with_https_response(
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
        "api.ip.sb",
        &path,
        REQUEST_TIMEOUT_MS,
        |request| {
            ensure_success_status(request)?;
            parse_ip_sb_location(&read_bounded_response(request, MAX_GEO_RESPONSE_BYTES)?)
        },
    )
}

fn measure_site_latency(site: NetworkSite) -> Result<Duration> {
    let (host, path) = site.endpoint();
    let started = Instant::now();
    with_https_response(
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
        host,
        path,
        LATENCY_TIMEOUT_MS,
        |_| Ok(started.elapsed()),
    )
    .with_context(|| format!("{}连接失败", site.name()))
}

fn with_https_response<T>(
    access_type: WINHTTP_ACCESS_TYPE,
    host: &str,
    path: &str,
    timeout_ms: i32,
    consume: impl FnOnce(*mut c_void) -> Result<T>,
) -> Result<T> {
    let host = null_terminated_utf16(host);
    let path = null_terminated_utf16(path);

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
            timeout_ms,
            timeout_ms,
            timeout_ms,
            timeout_ms,
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
            "创建网络请求失败",
        )?;

        WinHttpSendRequest(request.raw(), None, None, 0, 0, 0).context("发送网络请求失败")?;
        WinHttpReceiveResponse(request.raw(), ptr::null_mut()).context("接收网络响应失败")?;
        consume(request.raw())
    }
}

fn null_terminated_utf16(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn ensure_success_status(request: *mut c_void) -> Result<()> {
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

fn read_bounded_response(request: *mut c_void, max_bytes: usize) -> Result<Vec<u8>> {
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
        if response.len() > max_bytes {
            bail!("网络服务响应超过 {max_bytes} 字节");
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

#[derive(Debug, Deserialize)]
struct IpWhoisResponse {
    success: bool,
    message: Option<String>,
    country: Option<String>,
    region: Option<String>,
    city: Option<String>,
    connection: Option<IpWhoisConnection>,
}

#[derive(Debug, Deserialize)]
struct IpWhoisConnection {
    isp: Option<String>,
    org: Option<String>,
}

fn parse_ipwhois_location(response: &[u8]) -> Result<IpLocation> {
    let response: IpWhoisResponse =
        serde_json::from_slice(response).context("ipwho.is 返回了无效 JSON")?;
    if !response.success {
        bail!(
            "ipwho.is 查询失败: {}",
            response.message.as_deref().unwrap_or("未知错误")
        );
    }
    let network = response
        .connection
        .and_then(|connection| first_meaningful([connection.isp, connection.org]));
    IpLocation::from_parts(response.country, response.region, response.city, network)
}

#[derive(Debug, Deserialize)]
struct IpSbResponse {
    country: Option<String>,
    region: Option<String>,
    city: Option<String>,
    isp: Option<String>,
    organization: Option<String>,
    asn_organization: Option<String>,
    message: Option<String>,
}

fn parse_ip_sb_location(response: &[u8]) -> Result<IpLocation> {
    let response: IpSbResponse =
        serde_json::from_slice(response).context("IP.SB 返回了无效 JSON")?;
    let message = response.message.unwrap_or_else(|| "未返回归属地".into());
    let network = first_meaningful([
        response.isp,
        response.organization,
        response.asn_organization,
    ]);
    IpLocation::from_parts(response.country, response.region, response.city, network)
        .with_context(|| format!("IP.SB 查询失败: {message}"))
}

fn normalized(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn first_meaningful<const N: usize>(values: [Option<String>; N]) -> Option<String> {
    values.into_iter().find_map(normalized)
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
    fn ipwhois_location_keeps_only_meaningful_fields() {
        let response = r#"{
            "success": true,
            "message": null,
            "country": "中国",
            "region": "山东省",
            "city": "济南市",
            "connection": { "isp": "  ", "org": "中国移动" }
        }"#
        .as_bytes();

        assert_eq!(
            parse_ipwhois_location(response).unwrap(),
            IpLocation {
                country: Some("中国".into()),
                region: Some("山东省".into()),
                city: Some("济南市".into()),
                network: Some("中国移动".into()),
            }
        );
    }

    #[test]
    fn ipwhois_application_error_is_not_treated_as_a_location() {
        let response = br#"{
            "success": false,
            "message": "Reserved range",
            "country": null,
            "region": null,
            "city": null,
            "connection": null
        }"#;

        assert!(
            parse_ipwhois_location(response)
                .unwrap_err()
                .to_string()
                .contains("Reserved range")
        );
    }

    #[test]
    fn ip_sb_location_falls_back_to_the_asn_organization() {
        let response = br#"{
            "country": "Japan",
            "region": "Tokyo",
            "city": "Tokyo",
            "isp": null,
            "organization": null,
            "asn_organization": "Amazon.com, Inc.",
            "message": null
        }"#;

        assert_eq!(
            parse_ip_sb_location(response).unwrap().network.as_deref(),
            Some("Amazon.com, Inc.")
        );
    }

    #[test]
    fn network_site_catalog_is_balanced_and_unique() {
        let domestic = NetworkSite::ALL
            .iter()
            .filter(|site| site.scope() == "境内")
            .count();
        let hosts: std::collections::HashSet<_> = NetworkSite::ALL
            .iter()
            .map(|site| site.endpoint().0)
            .collect();

        assert_eq!(domestic, 4);
        assert_eq!(hosts.len(), NetworkSite::ALL.len());
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
