//! IP 工具页：逐项呈现三段出口、归属地与常用站点 HTTPS 响应耗时。
//!
//! 页面首次进入才发起查询，避免常驻托盘启动时产生无关网络请求。所有任务
//! 独立运行并逐项回填；刷新代次保证迟到的旧结果不会覆盖新一轮状态。

use std::{net::IpAddr, thread, time::Duration};

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use wisp_core::{
    IpKind, IpLocation, IpLocationLookup, IpLookup, IpService, NetworkSite, SiteLatencyLookup,
};

use crate::ui::tint;

const NETWORK_TINT: u32 = 0x1D9E75;

#[derive(Debug, Clone)]
enum LookupState {
    Idle,
    Loading,
    Ready {
        address: IpAddr,
        location: LocationState,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
enum LocationState {
    NotApplicable,
    Loading,
    Ready(IpLocation),
    Failed(String),
}

#[derive(Debug, Clone)]
enum LatencyState {
    Idle,
    Loading,
    Ready(Duration),
    Failed(String),
}

enum WorkerPayload {
    Address {
        kind: IpKind,
        result: IpLookup,
    },
    Location {
        kind: IpKind,
        result: IpLocationLookup,
    },
    Latency {
        site: NetworkSite,
        result: SiteLatencyLookup,
    },
}

struct WorkerMessage {
    generation: u64,
    payload: WorkerPayload,
}

pub(crate) struct IpView {
    focus_handle: FocusHandle,
    local: LookupState,
    direct: LookupState,
    proxy: LookupState,
    latencies: [LatencyState; NetworkSite::ALL.len()],
    generation: u64,
    loaded_once: bool,
    copied: Option<IpKind>,
}

impl IpView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            local: LookupState::Idle,
            direct: LookupState::Idle,
            proxy: LookupState::Idle,
            latencies: std::array::from_fn(|_| LatencyState::Idle),
            generation: 0,
            loaded_once: false,
            copied: None,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    /// 首次进入自动查询；之后保留结果，是否更新由用户明确触发。
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if !self.loaded_once {
            self.refresh(cx);
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.is_loading() {
            return;
        }

        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.loaded_once = true;
        self.local = LookupState::Loading;
        self.direct = LookupState::Loading;
        self.proxy = LookupState::Loading;
        self.latencies.fill(LatencyState::Loading);
        self.copied = None;
        cx.notify();

        let (result_tx, result_rx) = async_channel::unbounded();
        for kind in [IpKind::Local, IpKind::Direct, IpKind::Proxy] {
            let worker_tx = result_tx.clone();
            let failure_tx = result_tx.clone();
            let thread_name = match kind {
                IpKind::Local => "wisp-ip-local",
                IpKind::Direct => "wisp-ip-direct",
                IpKind::Proxy => "wisp-ip-proxy",
            };
            if let Err(error) = thread::Builder::new()
                .name(thread_name.into())
                .spawn(move || {
                    let result = IpService::lookup(kind);
                    let address = match &result {
                        IpLookup::Available(address) => Some(*address),
                        IpLookup::Unavailable(_) => None,
                    };
                    _ = worker_tx.send_blocking(WorkerMessage {
                        generation,
                        payload: WorkerPayload::Address { kind, result },
                    });
                    if kind != IpKind::Local
                        && let Some(address) = address
                    {
                        _ = worker_tx.send_blocking(WorkerMessage {
                            generation,
                            payload: WorkerPayload::Location {
                                kind,
                                result: IpService::locate(address),
                            },
                        });
                    }
                })
            {
                _ = failure_tx.try_send(WorkerMessage {
                    generation,
                    payload: WorkerPayload::Address {
                        kind,
                        result: IpLookup::Unavailable(format!("启动查询失败: {error}")),
                    },
                });
            }
        }
        for site in NetworkSite::ALL {
            let worker_tx = result_tx.clone();
            let failure_tx = result_tx.clone();
            if let Err(error) = thread::Builder::new()
                .name(format!("wisp-latency-{}", site.name()))
                .spawn(move || {
                    _ = worker_tx.send_blocking(WorkerMessage {
                        generation,
                        payload: WorkerPayload::Latency {
                            site,
                            result: IpService::measure(site),
                        },
                    });
                })
            {
                _ = failure_tx.try_send(WorkerMessage {
                    generation,
                    payload: WorkerPayload::Latency {
                        site,
                        result: SiteLatencyLookup::Unreachable(format!("启动探测失败: {error}")),
                    },
                });
            }
        }
        drop(result_tx);

        cx.spawn(async move |this, cx| {
            while let Ok(message) = result_rx.recv().await {
                this.update(cx, |this, cx| {
                    if this.generation != message.generation {
                        return;
                    }
                    match message.payload {
                        WorkerPayload::Address { kind, result } => {
                            *this.state_mut(kind) = match result {
                                IpLookup::Available(address) => LookupState::Ready {
                                    address,
                                    location: if kind == IpKind::Local {
                                        LocationState::NotApplicable
                                    } else {
                                        LocationState::Loading
                                    },
                                },
                                IpLookup::Unavailable(error) => LookupState::Failed(error),
                            };
                        }
                        WorkerPayload::Location { kind, result } => {
                            if let LookupState::Ready { location, .. } = this.state_mut(kind) {
                                *location = match result {
                                    IpLocationLookup::Available(location) => {
                                        LocationState::Ready(location)
                                    }
                                    IpLocationLookup::Unavailable(error) => {
                                        LocationState::Failed(error)
                                    }
                                };
                            }
                        }
                        WorkerPayload::Latency { site, result } => {
                            this.latencies[site_index(site)] = match result {
                                SiteLatencyLookup::Reachable(latency) => {
                                    LatencyState::Ready(latency)
                                }
                                SiteLatencyLookup::Unreachable(error) => {
                                    LatencyState::Failed(error)
                                }
                            };
                        }
                    }
                    cx.notify();
                })?;
            }
            anyhow::Ok(())
        })
        .detach();
    }

    fn state(&self, kind: IpKind) -> &LookupState {
        match kind {
            IpKind::Local => &self.local,
            IpKind::Direct => &self.direct,
            IpKind::Proxy => &self.proxy,
        }
    }

    fn state_mut(&mut self, kind: IpKind) -> &mut LookupState {
        match kind {
            IpKind::Local => &mut self.local,
            IpKind::Direct => &mut self.direct,
            IpKind::Proxy => &mut self.proxy,
        }
    }

    fn is_loading(&self) -> bool {
        let ip_is_loading = [IpKind::Local, IpKind::Direct, IpKind::Proxy]
            .into_iter()
            .any(|kind| {
                matches!(
                    self.state(kind),
                    LookupState::Loading
                        | LookupState::Ready {
                            location: LocationState::Loading,
                            ..
                        }
                )
            });
        ip_is_loading
            || self
                .latencies
                .iter()
                .any(|state| matches!(state, LatencyState::Loading))
    }

    fn copy(&mut self, kind: IpKind, address: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(address));
        self.copied = Some(kind);
        cx.notify();
    }

    fn metadata(&self, kind: IpKind) -> (&'static str, &'static str, IconName) {
        match kind {
            IpKind::Local => ("内网 IP", "当前活动网络", IconName::Network),
            IpKind::Direct => ("公网 IP", "直连网络出口", IconName::Globe),
            IpKind::Proxy => {
                let detail = match (&self.direct, &self.proxy) {
                    (
                        LookupState::Ready {
                            address: direct, ..
                        },
                        LookupState::Ready { address: proxy, .. },
                    ) if direct != proxy => "系统代理已生效",
                    (LookupState::Ready { .. }, LookupState::Ready { .. }) => "与直连出口一致",
                    _ => "系统代理出口",
                };
                ("代理出口 IP", detail, IconName::Globe)
            }
        }
    }

    fn render_row(&self, kind: IpKind, cx: &Context<Self>) -> Div {
        let (name, detail, icon) = self.metadata(kind);
        let icon_color = tint(NETWORK_TINT, cx);

        h_flex()
            .w_full()
            .min_h(px(72.))
            .px_3p5()
            .py_2()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.35))
            .child(
                div()
                    .size(px(34.))
                    .rounded_lg()
                    .bg(icon_color.opacity(0.12))
                    .text_color(icon_color)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(icon).w(px(18.)).h(px(18.))),
            )
            .child(
                v_flex()
                    .w(px(112.))
                    .gap_0p5()
                    .child(div().text_sm().font_medium().child(name))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(detail),
                    ),
            )
            .child(self.render_value(kind, cx))
    }

    fn render_value(&self, kind: IpKind, cx: &Context<Self>) -> Div {
        match self.state(kind) {
            LookupState::Idle => div()
                .flex_1()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("等待检测"),
            LookupState::Loading => div()
                .flex_1()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("检测中…"),
            LookupState::Failed(error) => div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_xs()
                .text_color(cx.theme().danger)
                .child(error.clone()),
            LookupState::Ready { address, location } => {
                let value = address.to_string();
                let copy_value = value.clone();
                let copied = self.copied == Some(kind);
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(div().text_lg().font_semibold().child(value))
                            .child(
                                Button::new(("copy-ip", kind_index(kind)))
                                    .ghost()
                                    .xsmall()
                                    .icon(if copied {
                                        IconName::Check
                                    } else {
                                        IconName::Copy
                                    })
                                    .tooltip(if copied { "已复制" } else { "复制 IP" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.copy(kind, copy_value.clone(), cx)
                                    })),
                            ),
                    )
                    .when(kind != IpKind::Local, |content| {
                        content.child(self.render_location(location, cx))
                    })
            }
        }
    }

    fn render_location(&self, state: &LocationState, cx: &Context<Self>) -> Div {
        let (text, color) = match state {
            LocationState::NotApplicable => (String::new(), cx.theme().muted_foreground),
            LocationState::Loading => ("归属地查询中…".into(), cx.theme().muted_foreground),
            LocationState::Ready(location) => {
                (format_location(location), cx.theme().muted_foreground)
            }
            LocationState::Failed(error) => (
                format!("归属地暂不可用 · {error}"),
                cx.theme().muted_foreground,
            ),
        };
        div()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_xs()
            .text_color(color)
            .child(text)
    }

    fn render_latency_card(&self, site: NetworkSite, cx: &Context<Self>) -> Div {
        let state = &self.latencies[site_index(site)];
        let (value, color) = match state {
            LatencyState::Idle => ("等待检测".into(), cx.theme().muted_foreground),
            LatencyState::Loading => ("检测中…".into(), cx.theme().muted_foreground),
            LatencyState::Ready(duration) => (format_latency(*duration), tint(NETWORK_TINT, cx)),
            LatencyState::Failed(error) => {
                let detail = concise_network_error(error);
                (format!("不可达 · {detail}"), cx.theme().danger)
            }
        };

        v_flex()
            .w(px(164.))
            .h(px(58.))
            .px_2p5()
            .py_2()
            .gap_1()
            .rounded_lg()
            .bg(cx.theme().secondary.opacity(0.42))
            .border_1()
            .border_color(cx.theme().border.opacity(0.28))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_xs().font_medium().child(site.name()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(site.scope()),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_sm()
                    .font_semibold()
                    .text_color(color)
                    .child(value),
            )
    }

    fn render_latency_panel(&self, cx: &Context<Self>) -> Div {
        v_flex()
            .px_3p5()
            .pt_2()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("常用站点延迟")
                    .child("HTTPS 响应"),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .flex_wrap()
                    .items_start()
                    .children(
                        NetworkSite::ALL
                            .into_iter()
                            .map(|site| self.render_latency_card(site, cx)),
                    ),
            )
    }
}

fn kind_index(kind: IpKind) -> usize {
    match kind {
        IpKind::Local => 0,
        IpKind::Direct => 1,
        IpKind::Proxy => 2,
    }
}

fn site_index(site: NetworkSite) -> usize {
    match site {
        NetworkSite::Baidu => 0,
        NetworkSite::NetEase => 1,
        NetworkSite::Aliyun => 2,
        NetworkSite::TencentCloud => 3,
        NetworkSite::GitHub => 4,
        NetworkSite::Google => 5,
        NetworkSite::YouTube => 6,
        NetworkSite::Amazon => 7,
    }
}

fn format_location(location: &IpLocation) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(4);
    for value in [
        &location.country,
        &location.region,
        &location.city,
        &location.network,
    ]
    .into_iter()
    .flatten()
    {
        if !parts.contains(&value.as_str()) {
            parts.push(value.as_str());
        }
    }
    parts.join(" · ")
}

fn format_latency(duration: Duration) -> String {
    match duration.as_millis() {
        0 => "<1 ms".into(),
        millis => format!("{millis} ms"),
    }
}

fn concise_network_error(error: &str) -> &'static str {
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("0x80072ee2")
        || normalized.contains("0x00002ee2")
        || normalized.contains("timed out")
        || error.contains("超时")
    {
        "超时"
    } else {
        "连接失败"
    }
}

impl Render for IpView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .child(
                h_flex()
                    .w_full()
                    .px_4()
                    .py_2p5()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.35))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("网络出口"),
                    )
                    .child(
                        Button::new("refresh-ip")
                            .outline()
                            .xsmall()
                            .icon(IconName::LoaderCircle)
                            .label("刷新")
                            .loading(self.is_loading())
                            .disabled(self.is_loading())
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .child(self.render_row(IpKind::Local, cx))
                    .child(self.render_row(IpKind::Direct, cx))
                    .child(self.render_row(IpKind::Proxy, cx)),
            )
            .child(self.render_latency_panel(cx))
            .child(div().flex_1())
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border.opacity(0.35))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("数据源：IPW · IPIP · ipify · ipwho.is · IP.SB"),
            )
    }
}
