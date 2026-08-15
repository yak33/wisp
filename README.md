# Wisp

> 轻若无物的 Windows 效率工具 —— 剪贴板历史 · 备忘快贴 · IP 工具

![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust)
![GPUI](https://img.shields.io/badge/UI-GPUI-8A2BE2)
![Platform](https://img.shields.io/badge/Platform-Windows%2011-0078D6?logo=windows)
![License](https://img.shields.io/badge/License-MIT-green)

Wisp（轻烟）是一个常驻托盘、快捷键瞬时唤起的桌面效率工具。它只做少数几件事，并把每一件做到**极致的快**：

- ⚡ **原生 GPU 渲染**：基于 [GPUI](https://www.gpui.rs/)（Zed 编辑器的 UI 框架）+ [gpui-component](https://github.com/longbridge/gpui-component)，无 WebView、无 JS 运行时
- 🪶 **常驻极轻**：实测常驻内存 ~27MB（debug 构建），后台 CPU 恒为 0%
- ⌨️ **键盘优先**：唤起即输入，↑↓ 选择、回车即得，全程无需鼠标
- 🔌 **事件驱动**：剪贴板监听走 `WM_CLIPBOARDUPDATE` 消息，零轮询

## 功能

### ✅ 剪贴板历史（已上线）

- 自动记录文本型剪贴板内容，重复复制不产生冗余条目（原条目自动回到顶部）
- 即时模糊搜索（中文友好，`%`/`_` 等通配符按字面匹配）
- 置顶收藏，置顶项恒排最前
- **回车直达粘贴**：自动还原焦点并模拟 Ctrl+V，内容直接落到你原来的输入框

### 🚧 规划中

- 图像与文件类剪贴板记录
- 备忘快贴：标签化文本片段库，搜索回车即粘贴
- IP 工具：内网 / 公网 / 代理出口三段 IP + 归属地 + 常用站点延迟面板

完整进度见 [docs/PROGRESS.md](docs/PROGRESS.md)。

## 交互

| 按键 | 行为 |
|------|------|
| `Alt+Space`* | 全局显示 / 隐藏 |
| 直接打字 | 搜索（唤起即聚焦，无需点击） |
| `↑` / `↓` | 选择条目 |
| `Enter` | 粘贴选中项到唤起前的窗口 |
| `Ctrl+Enter` | 仅复制到剪贴板，不粘贴 |
| `Ctrl+P` | 置顶 / 取消置顶选中项 |
| `Esc` / 失焦 | 隐藏窗口（失焦隐藏可关闭） |
| 托盘双击 | 唤起窗口 |

> \* 若 `Alt+Space` 已被其他工具（如 uTools）占用，自动降级为 `Ctrl+Alt+Space`，再降级为 ``Alt+` ``。实际生效的快捷键显示在窗口状态栏与托盘提示中。

## 架构

```
wisp
├── crates/core   wisp-core —— 纯 Rust 核心，与 UI 框架零耦合
│   ├── watcher   Win32 message-only 窗口，事件驱动监听剪贴板
│   ├── store     SQLite(WAL) 存储：指纹去重、置顶排序、模糊检索
│   ├── paste     粘贴链路：焦点还原 + SendInput 模拟 Ctrl+V
│   └── service   编排：监听线程 → 工作线程 → 壳层信号
└── crates/app    wisp-app —— GPUI 壳（bin: wisp）
    ├── main      窗口生命周期、托盘、全局快捷键、事件泵
    └── view      剪贴板视图：搜索、键盘导航、虚拟列表
```

核心设计取舍：

- **core 与 UI 解耦**：`wisp-core` 不知道 GPUI 的存在，未来更换壳层（或加 CLI）无需动核心；
- **窗口预创建 + 显隐**：唤起只是一次 `ShowWindow`，体感零延迟；
- **唤起即补查**：窗口每次激活自动重查一次，隐藏期间错过的变更信号不会丢；
- **虚拟列表**：单次查询上限 500 条，列表仅渲染可视区，数据量与帧率无关。

## 构建

```bash
# 依赖：Rust stable (MSVC toolchain)，Windows 10/11
cargo run            # 调试运行
cargo build --release
cargo test -p wisp-core
```

> 首次构建会从源码编译 GPUI 全家，耗时约 10 分钟；此后增量编译在秒级。

## 技术栈

| 层 | 选型 |
|----|------|
| UI 框架 | [GPUI](https://www.gpui.rs/)（Zed 同款，GPU 渲染） |
| 组件库 | [gpui-component](https://github.com/longbridge/gpui-component)（60+ 组件） |
| 存储 | rusqlite（bundled SQLite，WAL 模式） |
| 系统集成 | tray-icon · global-hotkey · windows-rs |
| 剪贴板读写 | arboard |

## License

[MIT](LICENSE) © ZHANGCHAO
