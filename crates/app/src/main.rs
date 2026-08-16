//! Wisp 入口：窗口生命周期、托盘、全局快捷键与事件处理。
//!
//! 交互约定：全局快捷键显隐（Alt+Space 被占用时自动降级）；
//! Esc 或失焦隐藏；托盘双击唤起，右键菜单退出。

mod assets;
mod clipboard_view;
mod config;
mod home_view;
mod memo_view;
mod wisp_view;

use std::{path::PathBuf, sync::Arc};

use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use gpui::*;
use gpui_component::Root;
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use tray_icon::{
    TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem},
};
use windows::{
    Win32::{
        Foundation::{ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError, HANDLE, HWND},
        System::{
            Registry::{
                HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, RegCloseKey, RegDeleteValueW,
                RegGetValueW, RegOpenKeyExW, RegSetValueExW, REG_SZ, RRF_RT_REG_SZ,
            },
            Threading::CreateMutexW,
        },
        UI::WindowsAndMessaging::{
            IsWindowVisible, SetForegroundWindow, ShowWindow, SW_HIDE, SW_SHOW,
        },
    },
    core::w,
};
use wisp_core::{ClipboardService, MemoService};

use crate::assets::WispAssets;

use crate::{config::Config, wisp_view::WispView};

const WINDOW_SIZE: Size<Pixels> = size(px(720.), px(520.));

// ==================== 进程级全局 ====================

/// 主窗口原生句柄。托盘/快捷键事件到达时窗口可能未激活，走 Win32 直控最稳。
struct NativeWindow(isize);

impl Global for NativeWindow {}

/// 实际注册成功的唤起快捷键标签（Alt+Space 可能被 uTools 等占用而降级）
pub(crate) struct WakeHotkey(pub &'static str);

impl Global for WakeHotkey {}

/// 唤起 Wisp 之前的前台窗口，粘贴时把焦点还给它
struct LastForeground(Option<isize>);

impl Global for LastForeground {}

/// 主视图句柄，供事件泵在剪贴板变更时触发刷新
struct MainView(Entity<WispView>);

impl Global for MainView {}

/// 托盘与快捷键管理器的生命周期必须覆盖整个进程，挂在 Global 上防止 drop；
/// 托盘句柄运行期还要用来重建菜单（勾选态刷新）
struct SystemIntegrations {
    tray: TrayIcon,
    _hotkeys: GlobalHotKeyManager,
}

impl Global for SystemIntegrations {}

// ==================== 原生窗口显隐 ====================

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut _)
}

fn hide_native(raw: isize) {
    unsafe {
        _ = ShowWindow(hwnd(raw), SW_HIDE);
    }
}

fn show_native(raw: isize) {
    unsafe {
        _ = ShowWindow(hwnd(raw), SW_SHOW);
        _ = SetForegroundWindow(hwnd(raw));
    }
}

fn is_native_visible(raw: isize) -> bool {
    unsafe { IsWindowVisible(hwnd(raw)).as_bool() }
}

pub(crate) fn hide_main_window(cx: &App) {
    if let Some(native) = cx.try_global::<NativeWindow>() {
        hide_native(native.0);
    }
}

// 窗口拖动由 GPUI 的 WindowControlArea::Drag 命中测试原生实现（见 wisp_view 头部），
// 无需手工投递 WM_NCLBUTTONDOWN / WM_SYSCOMMAND。

/// 唤起前记录的前台窗口，供粘贴时还原焦点
pub(crate) fn paste_target(cx: &App) -> Option<isize> {
    cx.try_global::<LastForeground>().and_then(|last| last.0)
}

// ==================== 托盘 ====================

fn tray_icon_image() -> tray_icon::Icon {
    // 品牌图标（favicon 变体 32px 渲染产物）；解码失败退回紫色圆点
    if let Ok(img) = image::load_from_memory(include_bytes!("../assets/tray.png")) {
        let rgba = img.to_rgba8();
        let (width, height) = (rgba.width(), rgba.height());
        if let Ok(icon) = tray_icon::Icon::from_rgba(rgba.into_raw(), width, height) {
            return icon;
        }
    }

    const SIZE: u32 = 32;
    let center = (SIZE / 2) as i32;
    let radius = center - 2;

    let rgba: Vec<u8> = (0..SIZE as i32)
        .flat_map(|y| (0..SIZE as i32).map(move |x| (x, y)))
        .flat_map(|(x, y)| {
            let (dx, dy) = (x - center, y - center);
            if dx * dx + dy * dy <= radius * radius {
                [0x6C, 0x5C, 0xE7, 0xFF] // Wisp 紫
            } else {
                [0, 0, 0, 0]
            }
        })
        .collect();

    tray_icon::Icon::from_rgba(rgba, SIZE, SIZE).expect("构建托盘图标失败")
}

// ==================== 壳层事件 ====================

/// 壳层事件：托盘/快捷键/菜单/剪贴板变更的统一信号，汇入单一协程处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellEvent {
    /// 切换显隐（快捷键 / 托盘菜单）
    Toggle,
    /// 仅唤起（托盘双击）
    Show,
    /// 剪贴板有新条目入库
    ClipsChanged,
    /// 切换开机自启（托盘菜单）
    ToggleAutostart,
    Quit,
}

// ==================== 单实例 ====================

/// 同名互斥体：已存在说明另一个 Wisp 在跑。双开会注册两套托盘/快捷键、
/// 双份剪贴板监听，第二个实例直接退出。句柄进程退出时由系统释放。
struct SingleInstance {
    _handle: HANDLE,
}

impl SingleInstance {
    fn acquire() -> Option<Self> {
        // Local\ 前缀：每用户会话独立命名空间，不跨会话互斥
        let handle = unsafe { CreateMutexW(None, false, w!("Local\\Wisp-SingleInstance")).ok()? };
        (unsafe { GetLastError() } != ERROR_ALREADY_EXISTS).then_some(Self { _handle: handle })
    }
}

// ==================== 开机自启 ====================

/// 探测 HKCU Run 键是否已注册自启（只关心存在性，不取值）。
fn autostart_enabled() -> bool {
    let mut needed: u32 = 0;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("Wisp"),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut needed),
        ) == ERROR_SUCCESS
    }
}

/// 写入/删除自启键。路径带引号，防止含空格的路径被启动器截断。
fn set_autostart(enabled: bool) -> bool {
    use std::os::windows::ffi::OsStrExt as _;

    // 先在安全代码里备好数据，注册表操作集中在 unsafe 块
    let wide: Vec<u16> = if enabled {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        let text = format!("\"{}\"", exe.display());
        let mut wide: Vec<u16> = std::ffi::OsStr::new(&text).encode_wide().collect();
        wide.push(0); // REG_SZ 要求 NUL 结尾
        wide
    } else {
        Vec::new()
    };

    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            None,
            KEY_SET_VALUE,
            &mut key,
        ) != ERROR_SUCCESS
        {
            return false;
        }

        let result = if enabled {
            let bytes =
                std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2);
            RegSetValueExW(key, w!("Wisp"), None, REG_SZ, Some(bytes))
        } else {
            RegDeleteValueW(key, w!("Wisp"))
        };
        _ = RegCloseKey(key);
        result == ERROR_SUCCESS
    }
}

/// 托盘菜单。勾选态取自注册表实况——切换后整体重建，绕开菜单项
/// 不可跨线程持有的问题（muda 句柄是 Rc）。
fn build_tray_menu(hotkey_label: &str) -> Menu {
    let menu = Menu::new();
    let toggle = MenuItem::with_id("toggle", format!("显示 / 隐藏（{hotkey_label}）"), true, None);
    let autostart = CheckMenuItem::with_id("autostart", "开机自启", true, autostart_enabled(), None);
    let quit = MenuItem::with_id("quit", "退出 Wisp", true, None);
    menu.append(&toggle)
        .and_then(|_| menu.append(&autostart))
        .and_then(|_| menu.append(&quit))
        .expect("托盘菜单构建失败");
    menu
}

// ==================== 入口 ====================

fn db_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Wisp")
        .join("wisp.db")
}

fn main() {
    // 已有实例在跑：静默退出，托盘与快捷键仍归首实例管
    let Some(_single_instance) = SingleInstance::acquire() else {
        return;
    };

    let app = gpui_platform::application().with_assets(WispAssets::new());

    app.run(move |cx| {
        gpui_component::init(cx);
        cx.activate(true);

        // 剪贴板服务：监听/入库在 core 的独立线程，变更信号进壳层事件泵
        let (changed_tx, changed_rx) = crossbeam_channel::unbounded::<()>();
        let db_path = db_path();
        let clipboard_service = Arc::new(
            ClipboardService::start(&db_path, changed_tx).expect("启动剪贴板服务失败"),
        );
        let memo_service = Arc::new(MemoService::open(&db_path).expect("打开备忘库失败"));
        // 与数据库同目录的轻量配置（上次页面等），进程重启后恢复
        let config = Config::load(&db_path.with_file_name("wisp.cfg"));

        // Alt+Space 大概率被 uTools 等工具占用，按候选顺序降级注册
        let hotkeys = GlobalHotKeyManager::new().expect("初始化全局快捷键失败");
        let candidates = [
            (HotKey::new(Some(Modifiers::ALT), Code::Space), "Alt+Space"),
            (
                HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space),
                "Ctrl+Alt+Space",
            ),
            (HotKey::new(Some(Modifiers::ALT), Code::Backquote), "Alt+`"),
        ];
        let hotkey_label = candidates
            .iter()
            .find(|(hotkey, _)| hotkeys.register(*hotkey).is_ok())
            .map(|(_, label)| *label)
            .expect("所有候选快捷键均注册失败");
        cx.set_global(WakeHotkey(hotkey_label));

        let tray = TrayIconBuilder::new()
            .with_tooltip(format!("Wisp — {hotkey_label} 唤起"))
            .with_icon(tray_icon_image())
            .with_menu(Box::new(build_tray_menu(hotkey_label)))
            .build()
            .expect("创建托盘图标失败");

        cx.set_global(SystemIntegrations {
            tray,
            _hotkeys: hotkeys,
        });

        let window_options = WindowOptions {
            titlebar: None,
            window_bounds: Some(WindowBounds::centered(WINDOW_SIZE, cx)),
            window_decorations: Some(WindowDecorations::Client),
            // 启动器形态：gpui 以 WS_EX_TOOLWINDOW | WS_EX_TOPMOST 创建——
            // 不进任务栏与 Alt+Tab，且悬浮于普通窗口之上（uTools 同款）
            kind: WindowKind::PopUp,
            ..Default::default()
        };

        cx.spawn(async move |cx| {
            cx.open_window(window_options, |window, cx| {
                if let Ok(handle) = window.window_handle() {
                    if let RawWindowHandle::Win32(win32) = handle.as_raw() {
                        cx.set_global(NativeWindow(win32.hwnd.get()));
                    }
                }
                let view = cx.new(|cx| {
                    WispView::new(
                        Arc::clone(&clipboard_service),
                        Arc::clone(&memo_service),
                        config,
                        window,
                        cx,
                    )
                });
                cx.set_global(MainView(view.clone()));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("打开主窗口失败");
        })
        .detach();

        // 事件处理：托盘/快捷键/菜单在各自 crate 的内部线程回调里投递，
        // 剪贴板变更由桥接线程转发；主侧单个协程 await 唤醒，全程无轮询
        let (event_tx, event_rx) = async_channel::unbounded::<ShellEvent>();

        GlobalHotKeyEvent::set_event_handler(Some({
            let event_tx = event_tx.clone();
            move |ev: GlobalHotKeyEvent| {
                if ev.state == HotKeyState::Pressed {
                    _ = event_tx.try_send(ShellEvent::Toggle);
                }
            }
        }));
        MenuEvent::set_event_handler(Some({
            let event_tx = event_tx.clone();
            move |ev: MenuEvent| match ev.id.0.as_str() {
                "toggle" => _ = event_tx.try_send(ShellEvent::Toggle),
                "autostart" => _ = event_tx.try_send(ShellEvent::ToggleAutostart),
                "quit" => _ = event_tx.try_send(ShellEvent::Quit),
                _ => {}
            }
        }));
        TrayIconEvent::set_event_handler(Some({
            let event_tx = event_tx.clone();
            move |ev: TrayIconEvent| {
                if matches!(ev, TrayIconEvent::DoubleClick { .. }) {
                    _ = event_tx.try_send(ShellEvent::Show);
                }
            }
        }));
        std::thread::Builder::new()
            .name("wisp-shell-bridge".into())
            .spawn(move || {
                for () in changed_rx.iter() {
                    _ = event_tx.try_send(ShellEvent::ClipsChanged);
                }
            })
            .expect("启动壳层桥接线程失败");

        cx.spawn(async move |cx| {
            while let Ok(event) = event_rx.recv().await {
                match event {
                    ShellEvent::Toggle => _ = cx.update(|cx| toggle_visibility(cx, true)),
                    ShellEvent::Show => _ = cx.update(|cx| toggle_visibility(cx, false)),
                    ShellEvent::ClipsChanged => _ = cx.update(|cx| {
                        if let Some(main) = cx.try_global::<MainView>() {
                            let view = main.0.clone();
                            view.update(cx, |view, cx| view.reload_clips(cx));
                        }
                    }),
                    ShellEvent::ToggleAutostart => _ = cx.update(|cx| {
                        set_autostart(!autostart_enabled());
                        // 菜单项句柄不可跨线程持有，重建整个菜单来刷新勾选态
                        if let Some(sys) = cx.try_global::<SystemIntegrations>() {
                            let label =
                                cx.try_global::<WakeHotkey>().map_or("快捷键", |k| k.0);
                            sys.tray
                                .set_menu(Some(Box::new(build_tray_menu(label))));
                        }
                    }),
                    ShellEvent::Quit => {
                        _ = cx.update(|cx| cx.quit());
                        return;
                    }
                }
            }
        })
        .detach();
    });
}

/// 显隐核心：`toggle` 为真且窗口可见则隐藏；否则唤起并聚焦。
fn toggle_visibility(cx: &mut App, toggle: bool) {
    let Some(native) = cx.try_global::<NativeWindow>() else {
        return;
    };
    let raw = native.0;
    if toggle && is_native_visible(raw) {
        hide_native(raw);
    } else {
        // 必须在自己抢到前台之前记录，否则记到的就是 Wisp 自己
        cx.set_global(LastForeground(wisp_core::capture_foreground(Some(raw))));
        show_native(raw);
        for window in cx.windows() {
            _ = window.update(cx, |_, window, _| window.activate_window());
        }
    }
}
