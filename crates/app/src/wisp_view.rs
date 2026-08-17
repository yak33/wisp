//! 应用根视图：主页 / 功能页导航与窗口级行为（聚焦、失焦隐藏、标题拖动）。
//!
//! 焦点、显隐、页面切换这类窗口级关注点集中在此处，子视图只负责各自的
//! 内容，避免多个子视图并存时互相抢焦点。

use std::sync::Arc;

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use wisp_core::{ClipboardService, MemoService};

use crate::{
    WakeHotkey,
    assets::WispIcon,
    clipboard_view::ClipboardView,
    config, hide_main_window,
    home_view::{HomeView, OpenFeature},
    ip_view::IpView,
    memo_view::MemoView,
    settings_view::SettingsView,
    theme::ThemePreference,
    ui::brand,
};

/// 一级页面。上次所在的功能页会持久化，重启后原样恢复；
/// 设置页刻意不参与恢复——唤起型工具重启后落在设置页很突兀。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Home,
    Clipboard,
    Memo,
    Ip,
    Settings,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Self::Home => "Wisp",
            Self::Clipboard => "剪贴板",
            Self::Memo => "备忘快贴",
            Self::Ip => "IP 工具",
            Self::Settings => "设置",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Clipboard => "clipboard",
            Self::Memo => "memo",
            Self::Ip => "ip",
            Self::Settings => "settings",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "home" => Some(Self::Home),
            "clipboard" => Some(Self::Clipboard),
            "memo" => Some(Self::Memo),
            "ip" => Some(Self::Ip),
            _ => None,
        }
    }
}

pub(crate) struct WispView {
    page: Page,
    home: Entity<HomeView>,
    clipboard: Entity<ClipboardView>,
    memos: Entity<MemoView>,
    ip: Entity<IpView>,
    settings: Entity<SettingsView>,
    auto_hide: bool,
    _subscriptions: Vec<Subscription>,
}

impl WispView {
    pub fn new(
        clipboard_service: Arc<ClipboardService>,
        memo_service: Arc<MemoService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let home = cx.new(|cx| HomeView::new(window, cx));
        let clipboard = cx.new(|cx| ClipboardView::new(clipboard_service, window, cx));
        let memos = cx.new(|cx| MemoView::new(memo_service, window, cx));
        let ip = cx.new(IpView::new);
        let settings = cx.new(|cx| SettingsView::new(window, cx));

        // 首帧之前激活上次的主题偏好，避免启动瞬间的明暗闪跳
        ThemePreference::current(cx).activate(Some(&mut *window), cx);

        let _subscriptions = vec![
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if window.is_window_active() {
                    this.focus_active_page(window, cx);
                    this.reload_active_page(cx);
                } else if this.auto_hide {
                    hide_main_window(cx);
                }
            }),
            // 系统深浅色切换（WM_SETTINGCHANGE / ImmersiveColorSet）：
            // 仅"跟随系统"档需要响应，显式选定亮/暗时用户意图优先
            cx.observe_window_appearance(window, |_, window, cx| {
                let preference = ThemePreference::current(cx);
                if preference == ThemePreference::System {
                    preference.activate(Some(window), cx);
                    cx.notify();
                }
            }),
            cx.subscribe_in(&home, window, |this: &mut Self, _, event, window, cx| {
                let page = match event {
                    OpenFeature::Clipboard => Page::Clipboard,
                    OpenFeature::Memo => Page::Memo,
                    OpenFeature::Ip => Page::Ip,
                };
                this.open_page(page, window, cx);
            }),
        ];

        let page = config::get("last_page", cx).and_then(|key| Page::from_key(&key));
        Self {
            page: page.unwrap_or(Page::Home),
            home,
            clipboard,
            memos,
            ip,
            settings,
            auto_hide: true,
            _subscriptions,
        }
    }

    /// 剪贴板有新内容时刷新——仅在该页可见时才有意义。
    pub fn reload_clips(&self, cx: &mut Context<Self>) {
        if self.page == Page::Clipboard {
            self.clipboard.update(cx, |view, cx| view.reload(cx));
        }
    }

    fn reload_active_page(&self, cx: &mut Context<Self>) {
        match self.page {
            Page::Home => self.home.update(cx, |view, cx| view.reload(cx)),
            Page::Clipboard => self.clipboard.update(cx, |view, cx| view.reload(cx)),
            Page::Memo => self.memos.update(cx, |view, cx| view.reload(cx)),
            Page::Ip => self.ip.update(cx, |view, cx| view.reload(cx)),
            // 设置页所有状态现读现显，无需刷新
            Page::Settings => {}
        }
    }

    fn focus_active_page(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.page {
            Page::Home => self
                .home
                .update(cx, |view, cx| view.focus_search(window, cx)),
            Page::Clipboard => self
                .clipboard
                .update(cx, |view, cx| view.focus_search(window, cx)),
            Page::Memo => self
                .memos
                .update(cx, |view, cx| view.focus_search(window, cx)),
            Page::Ip => self.ip.update(cx, |view, cx| view.focus(window, cx)),
            Page::Settings => self.settings.update(cx, |view, cx| view.focus(window, cx)),
        }
    }

    fn open_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        // 录制中切走页面等同取消：必须恢复全局唤起键
        if self.page == Page::Settings && page != Page::Settings {
            self.settings
                .update(cx, |view, cx| view.cancel_recording(cx));
        }
        self.page = page;
        if page != Page::Settings {
            config::set("last_page", page.key(), cx);
        }
        self.reload_active_page(cx);
        self.focus_active_page(window, cx);
        cx.notify();
    }

    /// 托盘菜单直达内置模块。统一复用正常页面切换流程，保证配置、刷新与焦点语义一致。
    pub(crate) fn open_page_from_shell(
        &mut self,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_page(page, window, cx);
    }

    /// 主题偏好循环一档（跟随系统 → 浅色 → 深色），立即生效并落盘。
    fn cycle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        ThemePreference::current(cx).next().select(Some(window), cx);
        cx.notify();
    }
}

impl Render for WispView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hotkey = cx
            .try_global::<WakeHotkey>()
            .map_or_else(|| "Alt+Space".to_string(), |k| k.label.clone());
        let brand_color = brand(cx);
        let theme = ThemePreference::current(cx);

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.modifiers.control {
                    match ev.keystroke.key.as_str() {
                        "1" => this.open_page(Page::Clipboard, window, cx),
                        "2" => this.open_page(Page::Memo, window, cx),
                        "3" => this.open_page(Page::Ip, window, cx),
                        "," => this.open_page(Page::Settings, window, cx),
                        _ => {}
                    }
                    return;
                }
                // Esc 逐层外退：功能页回主页，主页才隐藏窗口
                if ev.keystroke.key == "escape" {
                    match this.page {
                        Page::Home => hide_main_window(cx),
                        _ => this.open_page(Page::Home, window, cx),
                    }
                }
            }))
            .child(
                // 整个头部标记为窗口拖动区：WM_NCHITTEST 对该区域返回 HTCAPTION，Windows 原生接管移动。
                // 命中链默认会穿透 Normal 行为的子元素 hitbox，可点击的子元素必须 .occlude()
                // 截断命中链，否则点击被当成标题栏拖拽吞掉（Zed 的标题栏子元素同样如此处理）
                h_flex()
                    .px_3p5()
                    .py_2()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.35))
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .size(px(24.))
                                    .rounded_md()
                                    .bg(brand_color.opacity(0.15))
                                    .text_color(brand_color)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(WispIcon::Logo).w(px(12.)).h(px(18.))),
                            )
                            .when(self.page != Page::Home, |header| {
                                header
                                    .child(
                                        h_flex().occlude().child(
                                            Button::new("back-home")
                                                .ghost()
                                                .xsmall()
                                                .icon(IconName::ChevronLeft)
                                                .label("主页")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.open_page(Page::Home, window, cx)
                                                })),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground.opacity(0.6))
                                            .child("/"),
                                    )
                            })
                            .child(div().font_semibold().text_sm().child(self.page.title())),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(crate::ui::kbd_pill(hotkey, cx))
                            // 主题三态：跟随系统 / 浅色 / 深色，单按钮循环
                            .child(
                                h_flex().occlude().child(
                                    Button::new("cycle-theme")
                                        .ghost()
                                        .xsmall()
                                        .icon(theme.icon())
                                        .tooltip(theme.name())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.cycle_theme(window, cx);
                                        })),
                                ),
                            )
                            // 钉住 = 失焦不隐藏；未钉住（划掉）= 失焦自动隐藏
                            .child(
                                h_flex().occlude().child(
                                    Button::new("auto-hide")
                                        .ghost()
                                        .xsmall()
                                        .icon(if self.auto_hide {
                                            WispIcon::PinOff
                                        } else {
                                            WispIcon::Pin
                                        })
                                        .tooltip(if self.auto_hide {
                                            "失焦自动隐藏"
                                        } else {
                                            "已钉住"
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.auto_hide = !this.auto_hide;
                                            cx.notify();
                                        })),
                                ),
                            )
                            .child(
                                h_flex().occlude().child(
                                    Button::new("open-settings")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Settings)
                                        .tooltip("设置（Ctrl+,）")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_page(Page::Settings, window, cx)
                                        })),
                                ),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().map(|body| match self.page {
                Page::Home => body.child(self.home.clone()),
                Page::Clipboard => body.child(self.clipboard.clone()),
                Page::Memo => body.child(self.memos.clone()),
                Page::Ip => body.child(self.ip.clone()),
                Page::Settings => body.child(self.settings.clone()),
            }))
    }
}
