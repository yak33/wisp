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
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    VirtualListScrollHandle, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    h_flex,
    input::{InputEvent, InputState},
    menu::{ContextMenu, ContextMenuExt as _, PopupMenuItem},
    scroll::{ScrollableElement as _, ScrollbarAxis},
    tooltip::Tooltip,
    v_flex, v_virtual_list,
};
use wisp_core::{Clip, ClipFilter, ClipKind, ClipboardService};

use crate::{
    assets::WispIcon,
    hide_main_window, paste_target,
    ui::{
        brand, kbd_pill, search_input, selection_background, selection_background_subtle,
        selection_edge, warning,
    },
};

const ROW_HEIGHT: Pixels = px(40.);
const ROW_WIDTH: Pixels = px(700.);
const QUERY_LIMIT: usize = 500;
/// 悬停提示延迟：鼠标扫过列表时避免连续弹出
const TOOLTIP_DELAY: Duration = Duration::from_millis(300);
/// 悬停预览最多渲染的字符数。单条入库上限 2MB，正文只渲染截断预览，
/// 完整长度由尾注交代，防止超大内容卡住渲染。
const TOOLTIP_PREVIEW_CHARS: usize = 500;
/// 图像悬停放大预览的最长边（预览框 440×300，高 DPI 屏亦清晰）
const PREVIEW_MAX_EDGE: u32 = 640;
/// 放大预览缓存上限（条），超出整体清空——防长会话内存膨胀
const PREVIEW_CACHE_MAX: usize = 12;

pub(crate) struct ClipboardView {
    service: Arc<ClipboardService>,
    input_state: Entity<InputState>,
    keyword: String,
    filter: ClipFilter,
    items: Vec<Clip>,
    /// 全库未收藏记录数，随列表刷新更新；不受当前搜索和分类影响。
    unpinned_count: usize,
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
    /// 悬停放大预览缓存（解码原图压到 640px，见 [`Self::preview_image`]）
    previews: RefCell<HashMap<i64, Arc<RenderImage>>>,
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
            unpinned_count: 0,
            item_sizes: Rc::new(Vec::new()),
            selected: 0,
            selection: BTreeSet::new(),
            context_menu_row: None,
            thumbs: RefCell::new(HashMap::new()),
            previews: RefCell::new(HashMap::new()),
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
        self.unpinned_count = self.service.unpinned_count();
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
            if let Err(err) = self.service.copy_to_clipboard(clip.id) {
                eprintln!("仅复制失败: {err:#}");
            }
        }
    }

    /// 按 id 交付：粘贴到唤起前的窗口（无目标则退化为仅复制）。
    /// 交付全程在服务的独立线程执行（图像分支百毫秒级），UI 零阻塞。
    fn deliver_id(&mut self, id: i64, cx: &mut Context<Self>) {
        let target = paste_target(cx);
        hide_main_window(cx);
        self.service.paste_to(id, target);
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

    /// 清空全部未收藏历史。确认文案展示全库影响范围，不受当前筛选条件干扰。
    fn open_clear_history_dialog(&self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.service.unpinned_count();
        if count == 0 {
            return;
        }

        let view = cx.entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let view = view.clone();
            dialog
                .icon(Icon::new(IconName::TriangleAlert).text_color(warning(cx)))
                .title("清空剪贴板")
                .description(format!(
                    "将删除 {count} 条未收藏记录，收藏内容会保留。此操作无法撤销。"
                ))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("确认清空")
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text("取消")
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx| {
                    view.update(cx, |this, cx| match this.service.clear_unpinned() {
                        Ok(_) => {
                            this.thumbs.borrow_mut().clear();
                            this.previews.borrow_mut().clear();
                            this.reload(cx);
                            true
                        }
                        Err(err) => {
                            eprintln!("清空剪贴板历史失败: {err:#}");
                            false
                        }
                    })
                })
        });
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
        let bytes = thumb?;
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

    /// 悬停放大预览：解码原图并压到最长边 640——缩略图只有 128px，
    /// 硬拉到 440×300 的预览框必然发糊。含文件 IO，只在悬停时调用，
    /// 不进渲染热路径；缓存设上限防长会话膨胀。
    fn preview_image(&self, id: i64, path: &str) -> Option<Arc<RenderImage>> {
        if let Some(cached) = self.previews.borrow().get(&id) {
            return Some(cached.clone());
        }

        let bytes = std::fs::read(path).ok()?;
        let decoded = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let (width, height) = (decoded.width(), decoded.height());
        let scale = f64::from(PREVIEW_MAX_EDGE) / f64::from(width.max(height));
        let fitted = if scale < 1.0 {
            image::imageops::resize(
                &decoded,
                ((f64::from(width) * scale).round() as u32).max(1),
                ((f64::from(height) * scale).round() as u32).max(1),
                image::imageops::FilterType::Triangle,
            )
        } else {
            decoded
        };
        let (fit_w, fit_h) = (fitted.width(), fitted.height());

        let mut bgra = fitted.into_raw();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let buffer = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(fit_w, fit_h, bgra)
            .expect("预览数据与尺寸不符");
        let rendered = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
            buffer
        )]));

        let mut cache = self.previews.borrow_mut();
        if cache.len() >= PREVIEW_CACHE_MAX {
            cache.clear();
        }
        cache.insert(id, rendered.clone());
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
            .gap_2p5()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.3))
            .when(is_selected, |style| style.bg(selection_background(cx)))
            .when(!is_selected && in_set, |style| {
                style.bg(selection_background_subtle(cx))
            })
            .when(!is_selected && !in_set, |style| {
                style.hover(|style| style.bg(cx.theme().accent.opacity(0.3)))
            })
            // 左侧发光 Accent 导轨指示条
            .child(
                div()
                    .w(px(3.))
                    .h(px(16.))
                    .rounded_full()
                    .when(is_selected, |bar| bar.bg(selection_edge(cx)))
                    .when(!is_selected, |bar| bar.opacity(0.)),
            )
            .when(is_image, |row| {
                // 缩略图：28px 定高，object_fit Contain 保比例 + 微边框
                row.child(
                    div()
                        .flex_none()
                        .h(px(28.))
                        .w(px(28.))
                        .rounded_md()
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.4))
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
                        .rounded(px(4.))
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .bg(cx.theme().secondary.opacity(0.6))
                        .child(clip.kind.label()),
                )
            })
            .when(clip.pinned, |row| {
                row.child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(4.))
                        .text_xs()
                        .bg(warning(cx).opacity(0.15))
                        .text_color(warning(cx))
                        .child("置顶"),
                )
            })
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_sm()
                    .font_medium()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    // 文本行为首行截断；图像行为"宽×高 · 体积"摘要
                    .child(clip.preview.clone()),
            )
            .when(clip.kind == ClipKind::Text, |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground.opacity(0.8))
                        .child(format!("{} 字符", clip.char_count)),
                )
            })
            .child(
                div()
                    .w_16()
                    .text_right()
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
        // 图像行的 content 是落盘路径，悬停时按需解码原图放大显示
        let image_path = (clip.kind == ClipKind::Image).then(|| clip.content.clone());
        // 文件行的 content 是路径清单，悬停逐行列出文件名
        let file_names = (clip.kind == ClipKind::Files).then(|| {
            clip.content
                .lines()
                .map(|line| {
                    std::path::Path::new(line)
                        .file_name()
                        .map_or_else(|| line.to_string(), |name| name.to_string_lossy().into_owned())
                })
                .collect::<Vec<String>>()
        });
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
                        let entity = entity.clone();
                        let path = image_path.clone();
                        let files = file_names.clone();
                        let preview = preview.clone();
                        move |_, cx| {
                            // 图像：解码原图放大
                            if let Some(path) = path.as_deref()
                                && let Some(image) = entity.read(cx).preview_image(id, path)
                            {
                                return image_tooltip_body(image, preview.clone(), cx);
                            }
                            // 文件：逐行列出文件名
                            if let Some(files) = files.as_ref() {
                                return files_tooltip_body(files, cx);
                            }
                            clip_tooltip_body(preview.clone(), char_count, cx)
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
            .px_3p5()
            .py_2()
            .gap_2()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(cx.theme().border.opacity(0.4))
            .bg(cx.theme().secondary.opacity(0.85))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded(px(4.))
                            .text_xs()
                            .font_medium()
                            .bg(brand(cx))
                            // 徽章底色是主题前景的反色块，文字取背景色保证双档对比度
                            .text_color(cx.theme().background)
                            .child(format!("已选 {count} 项")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("点击反选 · 按 Esc 清空选择"),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("batch-delete")
                            .danger()
                            .xsmall()
                            .label("批量删除")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.delete_selected_batch(cx)),
                            ),
                    )
                    .child(
                        Button::new("batch-cancel")
                            .ghost()
                            .xsmall()
                            .label("取消选择")
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
            .px_3p5()
            .pb_1p5()
            .gap_1p5()
            .children(ClipFilter::ALL.iter().enumerate().map(|(ix, filter)| {
                let active = *filter == self.filter;
                let shortcut = format!("Alt+{}", ix + 1);
                h_flex()
                    .id(("clip-filter", ix))
                    .px_2p5()
                    .py_1()
                    .gap_1p5()
                    .items_center()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .when(active, |chip| {
                        chip.bg(cx.theme().accent)
                            .text_color(cx.theme().accent_foreground)
                            .font_medium()
                    })
                    .when(!active, |chip| {
                        chip.text_color(cx.theme().muted_foreground)
                            .bg(cx.theme().secondary.opacity(0.35))
                            .hover(|style| style.bg(cx.theme().accent.opacity(0.35)))
                    })
                    .child(filter.label())
                    .child(
                        div()
                            .text_xs()
                            .opacity(0.6)
                            .child(shortcut),
                    )
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
            .child(
                div()
                    .px_3p5()
                    .pt_3()
                    .pb_2()
                    .child(search_input(&self.input_state, cx)),
            )
            .child(self.render_filter_bar(cx))
            .child(
                h_flex()
                    .px_3p5()
                    .py_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .justify_between()
                    .items_center()
                    .border_t_1()
                    .border_color(cx.theme().border.opacity(0.35))
                    .child(format!("{} 条记录", self.items.len()))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("clear-history")
                                    .ghost()
                                    .small()
                                    .icon(WispIcon::Trash)
                                    .tooltip(if self.unpinned_count == 0 {
                                        "暂无可清空的历史"
                                    } else {
                                        "清空剪贴板"
                                    })
                                    .disabled(self.unpinned_count == 0)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_clear_history_dialog(window, cx)
                                    })),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(kbd_pill("↑↓", cx))
                                    .child("选择"),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(kbd_pill("↵", cx))
                                    .child("粘贴"),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(kbd_pill("Ctrl+P", cx))
                                    .child("收藏"),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(kbd_pill("Alt+1~5", cx))
                                    .child("分类"),
                            ),
                    ),
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
                                .child("没有匹配的记录"),
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

/// 文件条目的悬停预览：逐行列出文件名（上限 12 行），尾注交代总数。
fn files_tooltip_body(names: &[String], cx: &App) -> Div {
    const MAX_LINES: usize = 12;

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
                .children(names.iter().take(MAX_LINES).map(|name| div().child(name.clone()))),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(if names.len() > MAX_LINES {
                    format!("共 {} 个文件", names.len())
                } else {
                    format!("{} 个文件", names.len())
                }),
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
