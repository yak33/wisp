//! 剪贴板变更监听：专用线程上的 message-only 窗口接收 `WM_CLIPBOARDUPDATE`。
//!
//! 事件驱动而非轮询——监听线程绝大多数时间阻塞在 `GetMessageW`，
//! 常驻 CPU 恒为零，这是"极致性能"叙事里最重要的一块地基。

use std::{
    sync::OnceLock,
    thread::{self, JoinHandle},
};

use anyhow::{Context as _, Result, bail};
use crossbeam_channel::Sender;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{
            DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener},
            LibraryLoader::GetModuleHandleW,
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
            PostMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE,
            WINDOW_STYLE, WM_CLIPBOARDUPDATE, WM_CLOSE, WM_DESTROY, WNDCLASSW,
        },
    },
    core::w,
};

/// 单进程仅存在一个监听器，Sender 挂在进程级静态量上供 wnd_proc 取用。
static CLIPBOARD_TX: OnceLock<Sender<()>> = OnceLock::new();

pub(crate) struct ClipboardWatcher {
    hwnd: isize,
    thread: Option<JoinHandle<()>>,
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            if let Some(tx) = CLIPBOARD_TX.get() {
                _ = tx.try_send(());
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                _ = RemoveClipboardFormatListener(hwnd);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub(crate) fn start(tx: Sender<()>) -> Result<ClipboardWatcher> {
    if CLIPBOARD_TX.set(tx).is_err() {
        bail!("剪贴板监听器在一个进程内只允许启动一次");
    }

    let (hwnd_tx, hwnd_rx) = crossbeam_channel::bounded::<std::result::Result<isize, String>>(1);

    let thread = thread::Builder::new()
        .name("wisp-clipboard-watcher".into())
        .spawn(move || unsafe {
            let create = || -> Result<HWND> {
                let instance = GetModuleHandleW(None).context("GetModuleHandleW 失败")?;
                let class_name = w!("WispClipboardWatcher");

                let class = WNDCLASSW {
                    lpfnWndProc: Some(wnd_proc),
                    hInstance: instance.into(),
                    lpszClassName: class_name,
                    ..Default::default()
                };
                if RegisterClassW(&class) == 0 {
                    bail!("注册剪贴板监听窗口类失败");
                }

                let hwnd = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    class_name,
                    w!(""),
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE), // message-only：不参与渲染与 Z 序，仅收消息
                    None,
                    Some(instance.into()),
                    None,
                )
                .context("创建剪贴板监听窗口失败")?;

                AddClipboardFormatListener(hwnd).context("AddClipboardFormatListener 失败")?;
                Ok(hwnd)
            };

            match create() {
                Ok(hwnd) => {
                    _ = hwnd_tx.send(Ok(hwnd.0 as isize));
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                Err(err) => {
                    _ = hwnd_tx.send(Err(format!("{err:#}")));
                }
            }
        })
        .context("启动剪贴板监听线程失败")?;

    match hwnd_rx.recv() {
        Ok(Ok(hwnd)) => Ok(ClipboardWatcher {
            hwnd,
            thread: Some(thread),
        }),
        Ok(Err(message)) => bail!(message),
        Err(_) => bail!("剪贴板监听线程异常退出"),
    }
}

impl Drop for ClipboardWatcher {
    fn drop(&mut self) {
        unsafe {
            _ = PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(thread) = self.thread.take() {
            _ = thread.join();
        }
    }
}
