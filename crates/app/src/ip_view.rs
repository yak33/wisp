//! IP 工具页：逐项呈现内网、直连公网与系统代理出口。
//!
//! 页面首次进入才发起查询，避免常驻托盘启动时产生无关网络请求。三项任务
//! 独立运行并逐项回填；刷新代次保证迟到的旧结果不会覆盖新一轮状态。

use std::{net::IpAddr, thread};

use gpui::*;
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use wisp_core::{IpKind, IpLookup, IpService};

use crate::ui::tint;

const NETWORK_TINT: u32 = 0x1D9E75;

#[derive(Debug, Clone)]
enum LookupState {
    Idle,
    Loading,
    Ready(IpAddr),
    Failed(String),
}

pub(crate) struct IpView {
    focus_handle: FocusHandle,
    local: LookupState,
    direct: LookupState,
    proxy: LookupState,
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
        self.copied = None;
        cx.notify();

        let (result_tx, result_rx) = async_channel::unbounded();
        for kind in [IpKind::Local, IpKind::Direct, IpKind::Proxy] {
            let worker_tx = result_tx.clone();
            let fallback_tx = result_tx.clone();
            let thread_name = match kind {
                IpKind::Local => "wisp-ip-local",
                IpKind::Direct => "wisp-ip-direct",
                IpKind::Proxy => "wisp-ip-proxy",
            };
            if let Err(error) = thread::Builder::new()
                .name(thread_name.into())
                .spawn(move || {
                    _ = worker_tx.send_blocking((generation, kind, IpService::lookup(kind)));
                })
            {
                _ = fallback_tx.try_send((
                    generation,
                    kind,
                    IpLookup::Unavailable(format!("启动查询失败: {error}")),
                ));
            }
        }
        drop(result_tx);

        cx.spawn(async move |this, cx| {
            while let Ok((generation, kind, result)) = result_rx.recv().await {
                this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    *this.state_mut(kind) = match result {
                        IpLookup::Available(address) => LookupState::Ready(address),
                        IpLookup::Unavailable(error) => LookupState::Failed(error),
                    };
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
        [IpKind::Local, IpKind::Direct, IpKind::Proxy]
            .into_iter()
            .any(|kind| matches!(self.state(kind), LookupState::Loading))
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
                    (LookupState::Ready(direct), LookupState::Ready(proxy)) if direct != proxy => {
                        "系统代理已生效"
                    }
                    (LookupState::Ready(_), LookupState::Ready(_)) => "与直连出口一致",
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
            .min_h(px(98.))
            .px_4()
            .py_3()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.35))
            .child(
                div()
                    .size(px(38.))
                    .rounded_lg()
                    .bg(icon_color.opacity(0.12))
                    .text_color(icon_color)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(icon).w(px(20.)).h(px(20.))),
            )
            .child(
                v_flex()
                    .w(px(128.))
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
            LookupState::Ready(address) => {
                let value = address.to_string();
                let copy_value = value.clone();
                let copied = self.copied == Some(kind);
                h_flex()
                    .flex_1()
                    .min_w_0()
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
                    )
            }
        }
    }
}

fn kind_index(kind: IpKind) -> usize {
    match kind {
        IpKind::Local => 0,
        IpKind::Direct => 1,
        IpKind::Proxy => 2,
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
            .child(div().flex_1())
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border.opacity(0.35))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("数据源：IPW · IPIP · ipify · AWS"),
            )
    }
}
