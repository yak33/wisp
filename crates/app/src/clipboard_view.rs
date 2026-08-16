//! 剪贴板历史视图：搜索、键盘导航、回车直达粘贴。

use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{
    ActiveTheme as _, VirtualListScrollHandle,
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::{ScrollableElement as _, ScrollbarAxis},
    tooltip::Tooltip,
    v_flex, v_virtual_list,
};
use wisp_core::{Clip, ClipFilter, ClipboardService};

use crate::{hide_main_window, paste_target};

const ROW_HEIGHT: Pixels = px(40.);
const ROW_WIDTH: Pixels = px(700.);
const QUERY_LIMIT: usize = 500;
/// 悬停提示延迟：鼠标扫过列表时避免连续弹出
const TOOLTIP_DELAY: Duration = Duration::from_millis(300);
/// 悬停预览最多渲染的字符数。单条入库上限 2MB，正文只渲染截断预览，
/// 完整长度由尾注交代，防止超大内容卡住渲染。
const TOOLTIP_PREVIEW_CHARS: usize = 500;

pub(crate) struct ClipboardView {
    service: Arc<ClipboardService>,
    input_state: Entity<InputState>,
    keyword: String,
    filter: ClipFilter,
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
            filter: ClipFilter::All,
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

    /// 以当前分类与关键字重查列表。新内容置顶，故刷新后选中项归位到首条。
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.items = self.service.query(self.filter, &self.keyword, QUERY_LIMIT);
        self.item_sizes = Rc::new(vec![size(ROW_WIDTH, ROW_HEIGHT); self.items.len()]);
        self.selected = 0;
        self.scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn set_filter(&mut self, filter: ClipFilter, cx: &mut Context<Self>) {
        self.filter = filter;
        self.reload(cx);
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

    /// 渲染一个列表项：行 + 悬停预览 + 点击交付。
    fn render_item(&self, ix: usize, cx: &Context<Self>) -> Stateful<Div> {
        let clip = &self.items[ix];
        let preview = tooltip_preview(&clip.content, clip.char_count);
        let char_count = clip.char_count;

        self.render_row(ix, cx)
            .id(ix)
            .tooltip_show_delay(TOOLTIP_DELAY)
            .tooltip(move |window, cx| {
                Tooltip::element({
                    let preview = preview.clone();
                    move |_, cx| clip_tooltip_body(preview.clone(), char_count, cx)
                })
                .build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = ix;
                this.deliver_selected(true, cx);
            }))
    }

    /// 分类标签行：全部 / 文本 / 图像 / 文件 / 收藏（Alt+1~5 等效）
    fn render_filter_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .px_3()
            .pb_1()
            .gap_1()
            .children(ClipFilter::ALL.iter().enumerate().map(|(ix, filter)| {
                let active = *filter == self.filter;
                div()
                    .id(("clip-filter", ix))
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_xs()
                    .cursor_pointer()
                    .when(active, |chip| chip.bg(cx.theme().accent))
                    .when(!active, |chip| {
                        chip.text_color(cx.theme().muted_foreground)
                            .hover(|style| style.bg(cx.theme().accent.opacity(0.3)))
                    })
                    .child(filter.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_filter(*filter, cx)))
            }))
    }
}

impl Render for ClipboardView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                match ev.keystroke.key.as_str() {
                    // Esc 不在此消费，冒泡到根视图统一处理（回主页 / 隐藏窗口）
                    "up" => this.move_selection(-1, cx),
                    "down" => this.move_selection(1, cx),
                    "p" if ev.keystroke.modifiers.control => this.toggle_pin_selected(cx),
                    key if ev.keystroke.modifiers.alt => {
                        if let Some(n) = key.parse::<usize>().ok().filter(|n| (1..=5).contains(n)) {
                            this.set_filter(ClipFilter::ALL[n - 1], cx);
                        }
                    }
                    _ => {}
                }
            }))
            .child(div().px_3().pt_2().pb_1().child(Input::new(&self.input_state)))
            .child(self.render_filter_bar(cx))
            .child(
                h_flex()
                    .px_3()
                    .pb_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .justify_between()
                    .child(format!("{} 条记录", self.items.len()))
                    .child("↑↓ 选择 · 回车粘贴 · Alt+1~5 分类 · Ctrl+P 置顶"),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(self.items.is_empty(), |body| {
                        body.child(
                            div()
                                .p_6()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(match self.filter {
                                    ClipFilter::Image => "图像剪贴板尚未支持（M3 规划中）",
                                    ClipFilter::Files => "文件剪贴板尚未支持（M3 规划中）",
                                    _ => "没有匹配的记录",
                                }),
                        )
                    })
                    .when(!self.items.is_empty(), |body| {
                        body.child(
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
                                                        Some(this.render_item(ix, cx))
                                                    })
                                                    .collect()
                                            },
                                        )
                                        .track_scroll(&self.scroll_handle),
                                    )
                                    .scrollbar(&self.scroll_handle, ScrollbarAxis::Vertical),
                            ),
                        )
                    }),
            )
    }
}

/// 悬停预览正文：保留换行截断至 TOOLTIP_PREVIEW_CHARS 字符，超出补省略号。
fn tooltip_preview(content: &str, char_count: i64) -> String {
    let mut preview: String = content.chars().take(TOOLTIP_PREVIEW_CHARS).collect();
    if char_count as usize > TOOLTIP_PREVIEW_CHARS {
        preview.push('…');
    }
    preview
}

/// 悬停提示正文：截断预览 + 总字符数尾注。
///
/// 换行用逐行子元素保留（此版 gpui 的 WhiteSpace 无 pre-wrap），
/// 空行以空格占位维持行高。
fn clip_tooltip_body(preview: String, char_count: i64, cx: &App) -> Div {
    v_flex()
        .max_w(px(480.))
        .py_1()
        .gap_1()
        .child(
            v_flex()
                .min_w(px(320.))
                .max_h(px(320.))
                .overflow_hidden()
                .text_sm()
                .line_height(relative(1.5))
                .children(preview.lines().map(|line| {
                    div().child(if line.is_empty() { " ".to_string() } else { line.to_string() })
                })),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("共 {char_count} 字符")),
        )
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
