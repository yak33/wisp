//! 剪贴板历史视图：搜索、键盘导航、回车直达粘贴。

use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    rc::Rc,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{
    ActiveTheme as _, Sizable as _, VirtualListScrollHandle,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenu, ContextMenuExt as _, PopupMenuItem},
    scroll::{ScrollableElement as _, ScrollbarAxis},
    tooltip::Tooltip,
    v_flex, v_virtual_list,
};
use wisp_core::{Clip, ClipFilter, ClipKind, ClipboardService};

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
    /// 键盘光标行（↑↓/Enter/Ctrl+P 的作用对象）
    selected: usize,
    /// 鼠标单击累积的多选集合（clip id）。两条起进入批量模式，底部出现操作栏。
    selection: BTreeSet<i64>,
    /// 右键菜单正打开的行。该行悬停提示让位，避免两个浮层互相干扰；
    /// 鼠标离开该行或列表刷新时解除。
    context_menu_row: Option<i64>,
    /// 图像缩略图解码缓存（clip id → GPU 可渲染图像），避免每帧解码
    thumbs: RefCell<HashMap<i64, Arc<RenderImage>>>,
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
            selection: BTreeSet::new(),
            context_menu_row: None,
            thumbs: RefCell::new(HashMap::new()),
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

    /// 以当前分类与关键字重查列表。新内容置顶，故刷新后选中项归位到首条；
    /// 结果集变了，多选集合随之作废。
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.items = self.service.query(self.filter, &self.keyword, QUERY_LIMIT);
        self.item_sizes = Rc::new(vec![size(ROW_WIDTH, ROW_HEIGHT); self.items.len()]);
        self.selected = 0;
        self.selection.clear();
        self.context_menu_row = None;
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
        if paste {
            self.deliver_id(clip.id, cx);
        } else {
            hide_main_window(cx);
            _ = self.service.copy_to_clipboard(clip.id);
        }
    }

    /// 按 id 交付：粘贴到唤起前的窗口（无目标则退化为仅复制）。
    fn deliver_id(&mut self, id: i64, cx: &mut Context<Self>) {
        let target = paste_target(cx);
        hide_main_window(cx);
        _ = self.service.paste_to(id, target);
    }

    /// 单击选择：切换该条的多选状态，键盘光标随之移过去。
    /// 一条即普通选中，两条起进入批量模式（底部出现操作栏）。
    fn click_select(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(clip) = self.items.get(ix) else {
            return;
        };
        self.selected = ix;
        if !self.selection.remove(&clip.id) {
            self.selection.insert(clip.id);
        }
        cx.notify();
    }

    /// 删除多选集合中的全部条目。
    fn delete_selected_batch(&mut self, cx: &mut Context<Self>) {
        for id in std::mem::take(&mut self.selection) {
            _ = self.service.delete(id);
        }
        self.reload(cx);
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

    /// 解码缩略图为 GPU 可渲染图像并按条目缓存——每行每帧解码不可接受。
    fn thumb_image(&self, id: i64, thumb: Option<&[u8]>) -> Option<Arc<RenderImage>> {
        let Some(bytes) = thumb else {
            return None;
        };
        if let Some(cached) = self.thumbs.borrow().get(&id) {
            return Some(cached.clone());
        }

        let decoded = image::load_from_memory(bytes).ok()?.to_rgba8();
        let (width, height) = (decoded.width(), decoded.height());
        let mut bgra = decoded.into_raw();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2); // RenderImage 取 BGRA（同 gpui 的 svg 渲染管线）
        }
        let buffer = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(width, height, bgra)
            .expect("缩略图数据与尺寸不符");
        let rendered = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
            buffer
        )]));
        self.thumbs.borrow_mut().insert(id, rendered.clone());
        Some(rendered)
    }

    fn render_row(&self, ix: usize, cx: &Context<Self>) -> Div {
        let clip = &self.items[ix];
        let is_selected = ix == self.selected;
        let in_set = self.selection.contains(&clip.id);
        let is_image = clip.kind == ClipKind::Image;

        h_flex()
            .w_full()
            .h(ROW_HEIGHT)
            .px_3()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.4))
            .when(is_selected, |style| style.bg(cx.theme().accent))
            .when(!is_selected && in_set, |style| {
                style.bg(cx.theme().accent.opacity(0.5))
            })
            .when(!is_selected && !in_set, |style| {
                style.hover(|style| style.bg(cx.theme().accent.opacity(0.3)))
            })
            .when(is_image, |row| {
                // 缩略图：28px 定高，object_fit Contain 保比例
                row.child(
                    div()
                        .flex_none()
                        .h(px(28.))
                        .w(px(28.))
                        .rounded_sm()
                        .overflow_hidden()
                        .when_some(self.thumb_image(clip.id, clip.thumb.as_deref()), |cell, image| {
                            cell.child(img(image).size_full())
                        }),
                )
            })
            .when(!is_image, |row| {
                row.child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded_sm()
                        .text_xs()
                        .bg(cx.theme().secondary)
                        .child(clip.kind.label()),
                )
            })
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
                    // 文本行为首行截断；图像行为"宽×高 · 体积"摘要
                    .child(clip.preview.clone()),
            )
            .when(!is_image, |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{} 字符", clip.char_count)),
                )
            })
            .child(
                div()
                    .w_16()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(relative_time(clip.created_at)),
            )
    }

    /// 渲染一个列表项：行 + 悬停预览 + 单击选择/双击上屏 + 右键菜单 + 收藏拖动排序。
    fn render_item(&self, ix: usize, cx: &Context<Self>) -> ContextMenu<Stateful<Div>> {
        let clip = &self.items[ix];
        let (id, pinned, char_count) = (clip.id, clip.pinned, clip.char_count);
        let preview = tooltip_preview(&clip.content, char_count);
        // 图像行：content 是落盘路径，悬停显示放大图而非路径文本
        let thumb_rendered = if clip.kind == ClipKind::Image {
            self.thumb_image(id, clip.thumb.as_deref())
        } else {
            None
        };
        let ghost_text = clip.preview.clone();
        let entity = cx.entity();

        self.render_row(ix, cx)
            .id(ix)
            .tooltip_show_delay(TOOLTIP_DELAY)
            .tooltip({
                // 右键菜单打开期间该行让位（见 context_menu_row），离开该行即恢复
                let entity = entity.clone();
                move |window, cx| {
                    if entity.read(cx).context_menu_row == Some(id) {
                        return cx.new(|_| HiddenTooltip).into();
                    }
                    Tooltip::element({
                        let preview = preview.clone();
                        let thumb_rendered = thumb_rendered.clone();
                        move |_, cx| match &thumb_rendered {
                            Some(image) => {
                                image_tooltip_body(image.clone(), preview.clone(), cx)
                            }
                            None => clip_tooltip_body(preview.clone(), char_count, cx),
                        }
                    })
                    .build(window, cx)
                }
            })
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                // 双击上屏；单击只做选择，连续单击累积多选
                if ev.click_count() >= 2 {
                    this.deliver_id(id, cx);
                } else {
                    this.click_select(ix, cx);
                }
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                // 光标离开该行：右键菜单的让位标记解除，悬停提示恢复
                if !hovered && this.context_menu_row == Some(id) {
                    this.context_menu_row = None;
                    cx.notify();
                }
            }))
            .when(pinned, |row| {
                row.on_drag(PinnedDrag(id), {
                    let ghost_text = ghost_text.clone();
                    move |_, _, _, cx| {
                        cx.new(|_| DragGhost(SharedString::from(ghost_text.as_str())))
                    }
                })
                // 松手于某条收藏上 = 插到它前面；顶边高亮即插入位置指示
                .drag_over::<PinnedDrag>(|style, _, _, cx| {
                    style.border_t_1().border_color(cx.theme().accent)
                })
                .on_drop::<PinnedDrag>(cx.listener(
                    move |this, drag: &PinnedDrag, _, cx| {
                        if drag.0 != id {
                            _ = this.service.reorder_pinned(drag.0, Some(id));
                            this.reload(cx);
                        }
                    },
                ))
            })
            .context_menu(move |menu, _, cx| {
                // 菜单构建发生在打开瞬间，借此时机标记本行
                entity.update(cx, |view, _| view.context_menu_row = Some(id));
                let copy = entity.clone();
                let paste = entity.clone();
                let pin = entity.clone();
                let del = entity.clone();
                menu
                    .item(PopupMenuItem::new("复制").on_click(move |_, _, cx| {
                        copy.update(cx, |view, _| _ = view.service.copy_to_clipboard(id));
                    }))
                    .item(PopupMenuItem::new("执行粘贴").on_click(move |_, _, cx| {
                        paste.update(cx, |view, cx| view.deliver_id(id, cx));
                    }))
                    .item(
                        PopupMenuItem::new(if pinned { "取消收藏" } else { "收藏" }).on_click(
                            move |_, _, cx| {
                                pin.update(cx, |view, cx| {
                                    if view.service.toggle_pin(id).is_ok() {
                                        view.reload(cx);
                                    }
                                });
                            },
                        ),
                    )
                    .item(PopupMenuItem::separator())
                    .item(PopupMenuItem::new("删除").on_click(move |_, _, cx| {
                        del.update(cx, |view, cx| {
                            if view.service.delete(id).is_ok() {
                                view.reload(cx);
                            }
                        });
                    }))
            })
    }

    /// 批量操作栏：两条及以上被选中时出现在列表底部。
    fn render_selection_bar(&self, cx: &Context<Self>) -> impl IntoElement {
        let count = self.selection.len();

        h_flex()
            .px_3()
            .py_1p5()
            .gap_2()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("已选 {count} 条 · 再点一次取消 · Esc 清空")),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("batch-delete")
                            .danger()
                            .xsmall()
                            .label("删除所选")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.delete_selected_batch(cx)),
                            ),
                    )
                    .child(
                        Button::new("batch-cancel")
                            .ghost()
                            .xsmall()
                            .label("清空选择")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selection.clear();
                                cx.notify();
                            })),
                    ),
            )
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
                    // 多选时 Esc 先清多选；无多选则冒泡到根视图（回主页 / 隐藏窗口）
                    "escape" => {
                        if !this.selection.is_empty() {
                            this.selection.clear();
                            cx.notify();
                            cx.stop_propagation();
                        }
                    }
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
                    .child("↑↓ 选择 · 回车/双击上屏 · 单击多选 · 右键菜单 · 收藏拖动排序 · Alt+1~5 分类"),
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
                                    ClipFilter::Files => "文件剪贴板尚未支持（规划中）",
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
            .when(self.selection.len() >= 2, |this| {
                this.child(self.render_selection_bar(cx))
            })
    }
}

/// 收藏条目拖动排序的载荷（clip id）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PinnedDrag(i64);

/// 拖动时跟随光标的小标签。
struct DragGhost(SharedString);

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .text_sm()
            .max_w(px(320.))
            .overflow_hidden()
            .bg(cx.theme().accent)
            .text_color(white())
            .child(self.0.clone())
    }
}

/// 右键菜单打开期间的悬停提示占位：渲染为空，视觉上等于不弹出。
struct HiddenTooltip;

impl Render for HiddenTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// 图像条目的悬停预览：放大图（Contain 保比例）+ 尺寸/体积尾注。
fn image_tooltip_body(image: Arc<RenderImage>, meta: String, cx: &App) -> Div {
    v_flex()
        .py_1()
        .gap_1()
        .child(
            div()
                .w(px(440.))
                .h(px(300.))
                .overflow_hidden()
                .rounded_sm()
                .child(img(image).size_full()),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(meta),
        )
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
