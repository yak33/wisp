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
    WakeHotkey, assets::WispIcon, clipboard_view::ClipboardView, config::Config,
    hide_main_window, home_view::{HomeView, OpenFeature}, memo_view::MemoView,
};

/// 一级页面。上次所在页面会持久化，重启后原样恢复。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Home,
    Clipboard,
    Memo,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Self::Home => "Wisp",
            Self::Clipboard => "剪贴板",
            Self::Memo => "备忘快贴",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Clipboard => "clipboard",
            Self::Memo => "memo",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "home" => Some(Self::Home),
            "clipboard" => Some(Self::Clipboard),
            "memo" => Some(Self::Memo),
            _ => None,
        }
    }
}

pub(crate) struct WispView {
    page: Page,
    config: Config,
    home: Entity<HomeView>,
    clipboard: Entity<ClipboardView>,
    memos: Entity<MemoView>,
    auto_hide: bool,
    _subscriptions: Vec<Subscription>,
}

impl WispView {
    pub fn new(
        clipboard_service: Arc<ClipboardService>,
        memo_service: Arc<MemoService>,
        config: Config,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let home = cx.new(|cx| HomeView::new(window, cx));
        let clipboard = cx.new(|cx| ClipboardView::new(clipboard_service, window, cx));
        let memos = cx.new(|cx| MemoView::new(memo_service, window, cx));

        let _subscriptions = vec![
            cx.observe_window_activation(
                window,
                |this: &mut Self, window, cx| {
                    if window.is_window_active() {
                        this.focus_active_page(window, cx);
                        this.reload_active_page(cx);
                    } else if this.auto_hide {
                        hide_main_window(cx);
                    }
                },
            ),
            cx.subscribe_in(&home, window, |this: &mut Self, _, event, window, cx| {
                let page = match event {
                    OpenFeature::Clipboard => Page::Clipboard,
                    OpenFeature::Memo => Page::Memo,
                };
                this.open_page(page, window, cx);
            }),
        ];

        let page = config.get("last_page").and_then(Page::from_key);
        Self {
            page: page.unwrap_or(Page::Home),
            config,
            home,
            clipboard,
            memos,
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
        }
    }

    fn open_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.config.set("last_page", page.key());
        self.reload_active_page(cx);
        self.focus_active_page(window, cx);
        cx.notify();
    }
}

impl Render for WispView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hotkey = cx.try_global::<WakeHotkey>().map_or("快捷键", |k| k.0);

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.modifiers.control {
                    match ev.keystroke.key.as_str() {
                        "1" => this.open_page(Page::Clipboard, window, cx),
                        "2" => this.open_page(Page::Memo, window, cx),
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
                    .px_4()
                    .pt_3()
                    .items_center()
                    .justify_between()
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .when(self.page != Page::Home, |header| {
                                header.child(
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
                            })
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            // 品牌图形 + 页面标题（标题与小字仍按基线对齐）
                            .child(Icon::new(WispIcon::Logo).w(px(14.)).h(px(21.)))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_baseline()
                                    .child(div().font_semibold().child(self.page.title()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("{hotkey} 显隐 · 按住此处拖动")),
                                    ),
                            ),
                    ),
                    )
                    .child(
                        // 钉住 = 失焦不隐藏；未钉住（划掉）= 失焦自动隐藏
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
                                    "失焦自动隐藏中 · 点击钉住，失焦不隐藏"
                                } else {
                                    "已钉住，失焦不隐藏 · 点击恢复自动隐藏"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.auto_hide = !this.auto_hide;
                                    cx.notify();
                                })),
                        ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .map(|body| match self.page {
                        Page::Home => body.child(self.home.clone()),
                        Page::Clipboard => body.child(self.clipboard.clone()),
                        Page::Memo => body.child(self.memos.clone()),
                    }),
            )
    }
}
