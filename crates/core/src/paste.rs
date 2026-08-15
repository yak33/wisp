//! 粘贴链路：把内容直接送回用户原来所在的窗口。
//!
//! 时序必须严格保持，任何一步错位都会粘到错误的窗口：
//!
//! 1. **显示 Wisp 之前**记录前台窗口（此后前台就是 Wisp 自己，再记就晚了）；
//! 2. 内容写入剪贴板；
//! 3. 隐藏 Wisp 窗口；
//! 4. 把前台还给目标窗口，等焦点落定；
//! 5. 模拟 Ctrl+V。
//!
//! 4、5 两步带阻塞等待，放在独立线程执行，不占用 UI 线程。

use std::{mem::size_of, thread, time::Duration};

use windows::Win32::{
    Foundation::HWND,
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
            KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN,
            VK_SHIFT, VK_V,
        },
        WindowsAndMessaging::{GetForegroundWindow, IsWindow, SetForegroundWindow},
    },
};

/// 让出前台后留给目标窗口的落定时间。低于此值在部分应用（Office、Electron）上会丢键。
const FOCUS_SETTLE: Duration = Duration::from_millis(80);
/// 按键序列之间的间隔，模拟人类击键节奏，避免目标应用来不及处理。
const KEYSTROKE_GAP: Duration = Duration::from_millis(20);

/// 记录当前前台窗口，供稍后还原焦点。
///
/// 必须在显示 Wisp 窗口**之前**调用；`self_hwnd` 用于排除 Wisp 自身
/// （窗口已可见时再次唤起会把自己记成目标）。
pub fn capture_foreground(self_hwnd: Option<isize>) -> Option<isize> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_invalid() {
        return None;
    }

    let raw = foreground.0 as isize;
    (Some(raw) != self_hwnd).then_some(raw)
}

/// 将剪贴板内容粘贴到目标窗口：还原焦点后模拟 Ctrl+V。
///
/// 假定内容已在剪贴板中。目标窗口已失效时静默放弃粘贴——
/// 内容仍在剪贴板里，用户手动 Ctrl+V 即可，不算失败。
pub fn paste_into(target: isize) {
    thread::spawn(move || {
        let hwnd = HWND(target as *mut _);
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return;
        }

        unsafe {
            // 当前进程是前台进程，因此有权把前台让给别人
            _ = SetForegroundWindow(hwnd);
        }
        thread::sleep(FOCUS_SETTLE);

        release_stuck_modifiers();
        send_ctrl_v();
    });
}

/// 抬起仍被物理按住的修饰键。
///
/// 唤起快捷键含 Alt/Ctrl，若用户尚未松手就回车，残留的修饰键会把
/// Ctrl+V 变成 Ctrl+Alt+V 之类的组合，落到目标应用上就是另一个命令。
fn release_stuck_modifiers() {
    let stuck: Vec<INPUT> = [VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
        .into_iter()
        .filter(|&vk| is_physically_down(vk))
        .map(|vk| key_event(vk, true))
        .collect();

    if !stuck.is_empty() {
        send(&stuck);
        thread::sleep(KEYSTROKE_GAP);
    }
}

fn send_ctrl_v() {
    send(&[key_event(VK_CONTROL, false), key_event(VK_V, false)]);
    thread::sleep(KEYSTROKE_GAP);
    send(&[key_event(VK_V, true), key_event(VK_CONTROL, true)]);
}

fn is_physically_down(vk: VIRTUAL_KEY) -> bool {
    // 高位为 1 表示按键当前处于按下状态
    unsafe { GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000 != 0 }
}

fn key_event(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, size_of::<INPUT>() as i32);
    }
}
