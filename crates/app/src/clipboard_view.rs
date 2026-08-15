//! 剪贴板历史视图：搜索、键盘导航、回车直达粘贴。

use std::{
    rc::Rc,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{
    ActiveTheme as _, VirtualListScrollHandle,
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::{ScrollableElement as _, ScrollbarAxis},
    v_flex, v_virtual_list,
};
use wisp_core::{Clip, ClipboardService};

use crate::{hide_main_window, paste_target};

const ROW_HEIGHT: Pixels = px(40.);
const ROW_WIDTH: Pixels = px(700.);
const QUERY_LIMIT: usize = 500;

pub(crate) struct ClipboardView {
    service: Arc<ClipboardService>,
    input_state: Entity<InputState>,
    keyword: String,
    items: Vec<Clip>,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    selected: usize,
    scroll_handle: VirtualListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl ClipboardView {
    pub fn new(service: Arc<ClipboardService>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input_state =
            cx.new(|cx| InputState::new(window, cx).placeholder("搜索剪贴板历史，回车粘贴选中项…"));

        let _subscriptions = vec![cx.subscribe_in(
            &input_state,
            window,
            |this: &mut Self, state, ev, _, cx| match ev {
                InputEvent::Change => {
                    this.keyword = state.read(cx).value().to_string();
                    this.reload(cx);
                }
                // Ctrl+Enter 只回填剪贴板，留给用户自己决定粘到哪
                InputEvent::PressEnter { secondary, .. } => this.deliver_selected(!secondary, cx),
                _ => {}
            },
        )];

        let mut view = Self {
            service,
            input_state,
            keyword: String::new(),
            items: Vec::new(),
            item_sizes: Rc::new(Vec::new()),
            selected: 0,
            scroll_handle: VirtualListScrollHandle::new(),
            _subscriptions,
        };
        view.reload(cx);
        view
    }

    pub fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input_state
            .update(cx, |state, cx| state.focus(window, cx));
    }

    /// 以当前关键字重查列表。新内容置顶，故刷新后选中项归位到首条。
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.items = self.service.query(&self.keyword, QUERY_LIMIT);
        self.item_sizes = Rc::new(vec![size(ROW_WIDTH, ROW_HEIGHT); self.items.len()]);
        self.selected = 0;
        self.scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// 交付选中项：`paste` 为真时直接粘贴到唤起前的窗口，否则仅回填剪贴板。
    ///
    /// 两种情形都先隐藏窗口——粘贴链路依赖 Wisp 让出前台。
    fn deliver_selected(&mut self, paste: bool, cx: &mut Context<Self>) {
        let Some(clip) = self.items.get(self.selected) else {
            return;
        };
        let id = clip.id;
        let target = paste.then(|| paste_target(cx)).flatten();

        hide_main_window(cx);
        _ = self.service.paste_to(id, target);
    }

    fn move_selection(&mut self, delta: i64, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() as i64 - 1;
        self.selected = (self.selected as i64 + delta).clamp(0, last) as usize;
        self.scroll_handle
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn toggle_pin_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(clip) = self.items.get(self.selected) {
            let id = clip.id;
            let keep = self.selected;
            if self.service.toggle_pin(id).is_ok() {
                self.reload(cx);
                self.selected = keep.min(self.items.len().saturating_sub(1));
            }
        }
    }

    fn render_row(&self, ix: usize, cx: &Context<Self>) -> Div {
        let clip = &self.items[ix];
        let is_selected = ix == self.selected;

        h_flex()
            .w_full()
            .h(ROW_HEIGHT)
            .px_3()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.4))
            .when(is_selected, |style| style.bg(cx.theme().accent))
            .when(!is_selected, |style| {
                style.hover(|style| style.bg(cx.theme().accent.opacity(0.3)))
            })
            .child(
                div()
                    .px_1p5()
                    .py_0p5()
                    .rounded_sm()
                    .text_xs()
                    .bg(cx.theme().secondary)
                    .child(clip.kind.label()),
            )
            .when(clip.pinned, |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("置顶"),
                )
            })
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_sm()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(clip.preview.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{} 字符", clip.char_count)),
            )
            .child(
                div()
                    .w_16()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(relative_time(clip.created_at)),
            )
    }
}

impl Render for ClipboardView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                match ev.keystroke.key.as_str() {
                    "escape" => hide_main_window(cx),
                    "up" => this.move_selection(-1, cx),
                    "down" => this.move_selection(1, cx),
                    "p" if ev.keystroke.modifiers.control => this.toggle_pin_selected(cx),
                    _ => {}
                }
            }))
            .child(div().px_3().pt_2().pb_1().child(Input::new(&self.input_state)))
            .child(
                h_flex()
                    .px_3()
                    .pb_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .justify_between()
                    .child(format!("{} 条记录", self.items.len()))
                    .child("↑↓ 选择 · 回车粘贴 · Ctrl+回车仅复制 · Ctrl+P 置顶"),
            )
            .child(
                div().flex_1().min_h_0().child(
                    div().relative().size_full().child(
                        v_flex()
                            .id("clip-list")
                            .relative()
                            .size_full()
                            .child(
                                v_virtual_list(
                                    cx.entity().clone(),
                                    "clip-items",
                                    self.item_sizes.clone(),
                                    |this, visible_range, _, cx| {
                                        visible_range
                                            .filter_map(|ix| {
                                                if ix >= this.items.len() {
                                                    return None;
                                                }
                                                Some(this.render_row(ix, cx).id(ix).on_click(
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.selected = ix;
                                                        this.deliver_selected(true, cx);
                                                    }),
                                                ))
                                            })
                                            .collect()
                                    },
                                )
                                .track_scroll(&self.scroll_handle),
                            )
                            .scrollbar(&self.scroll_handle, ScrollbarAxis::Vertical),
                    ),
                ),
            )
    }
}

fn relative_time(created_at_ms: i64) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let minutes = (now_ms - created_at_ms).max(0) / 60_000;

    match minutes {
        0 => "刚刚".into(),
        1..=59 => format!("{minutes} 分钟前"),
        60..=1439 => format!("{} 小时前", minutes / 60),
        _ => format!("{} 天前", minutes / 1440),
    }
}
