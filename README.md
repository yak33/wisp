# Wisp

![Wisp](docs/banner.png)

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

### ✅ 主页导航（已上线）

- uTools 风格：唤起先落主页，功能以图标网格呈现，点击进入对应功能页
- 搜索框过滤功能（中文名 / 英文别名），↑↓ 选择、回车即入，全程键盘可达
- 规划中的功能置灰占位，路线图内置可见

### ✅ 剪贴板历史（已上线）

- 自动记录文本型剪贴板内容，重复复制不产生冗余条目（原条目自动回到顶部）
- 即时模糊搜索（中文友好，`%`/`_` 等通配符按字面匹配）
- 分类筛选：全部 / 文本 / 图像 / 文件 / 收藏，点击或 `Alt+1~5` 切换（图像与文件随 M3 落地）
- 置顶收藏，置顶项恒排最前
- 悬停预览完整内容：保留换行截断 500 字符，尾注显示总字符数
- 自动保留策略：未置顶条目 90 天过期、总量 2000 条封顶，置顶豁免
- **回车直达粘贴**：自动还原焦点并模拟 Ctrl+V，内容直接落到你原来的输入框

### ✅ 备忘快贴（已上线）

- 标签化的文本片段库，常用话术 / 密钥 / 模板随手存取
- 左侧标签侧栏带计数，支持「全部 / 无标签 / 指定标签」筛选
- 搜索同时匹配内容与备注；选中回车直接粘贴
- 悬停预览完整内容，尾注附备注 / 标签 / 总字符数
- 在某标签下新建时自动预填该标签，连续录入不用重复输入

### 🚧 规划中

- 图像与文件类剪贴板记录
- IP 工具：内网 / 公网 / 代理出口三段 IP + 归属地 + 常用站点延迟面板

完整进度见 [docs/PROGRESS.md](docs/PROGRESS.md)。

## 交互

| 按键 / 操作 | 行为 |
|------|------|
| `Alt+Space`* | 全局显示 / 隐藏，唤起回到上次所在页面（跨重启记忆） |
| 直接打字 | 搜索（唤起即聚焦；主页过滤功能，功能页过滤条目） |
| `Enter` / 双击 | 主页进入功能；剪贴板条目粘贴到唤起前的窗口（备忘同） |
| 单击条目 | 选中；连续单击累积多选，两条起底部出现批量删除栏 |
| 右键条目 | 菜单：复制 / 执行粘贴 / 收藏 / 删除（剪贴板） |
| 拖动收藏条目 | 收藏组内手动排序：拖到某条上松手即插到它前面（剪贴板） |
| `↑` / `↓` | 选择功能 / 条目 |
| `Ctrl+1` / `Ctrl+2` | 任意页直达剪贴板 / 备忘快贴 |
| `Alt+1` ~ `Alt+5` | 剪贴板分类：全部 / 文本 / 图像 / 文件 / 收藏 |
| `Esc` | 逐层外退：清空多选 → 编辑态取消编辑 → 功能页回主页 → 主页隐藏窗口 |
| `Ctrl+Enter` | 仅复制到剪贴板，不粘贴 |
| `Ctrl+P` | 置顶 / 取消置顶（剪贴板） |
| `Ctrl+N` / `Ctrl+E` | 新建 / 编辑备忘（备忘快贴） |
| `Ctrl+S` | 保存备忘（编辑态） |
| 按住顶部标题区拖动 | 移动窗口 |
| 失焦 | 隐藏窗口（可在标题栏右侧关闭） |
| 托盘双击 | 唤起窗口 |

> \* 若 `Alt+Space` 已被其他工具（如 uTools）占用，自动降级为 `Ctrl+Alt+Space`，再降级为 ``Alt+` ``。实际生效的快捷键显示在窗口状态栏与托盘提示中。

## 架构

```
wisp
├── crates/core   wisp-core —— 纯 Rust 核心，与 UI 框架零耦合
│   ├── watcher   Win32 message-only 窗口，事件驱动监听剪贴板
│   ├── store     剪贴板存储：指纹去重、置顶排序、模糊检索
│   ├── memo      备忘存储：片段 / 标签多对多，事务化保存
│   ├── paste     粘贴链路：焦点还原 + SendInput 模拟 Ctrl+V
│   └── service   编排：监听线程 → 工作线程 → 壳层信号
└── crates/app    wisp-app —— GPUI 壳（bin: wisp）
    ├── main      窗口生命周期、托盘、全局快捷键、单实例与事件处理
    └── views     根视图（页面导航 / 焦点 / 显隐）+ 主页网格 + 剪贴板视图 + 备忘视图
```

核心设计取舍：

- **core 与 UI 解耦**：`wisp-core` 不知道 GPUI 的存在，未来更换壳层（或加 CLI）无需动核心；
- **窗口级关注点收口于根视图**：唤起聚焦、失焦隐藏、页面切换统一由 `WispView` 处理，子视图并存时不会互相抢焦点；
- **主页先行**：唤起落在功能网格而非某个具体功能，新功能只加一行表项，导航结构零改动；
- **窗口预创建 + 显隐**：唤起只是一次 `ShowWindow`，体感零延迟；
- **唤起即补查**：窗口每次激活自动重查一次，隐藏期间错过的变更信号不会丢；
- **虚拟列表**：列表仅渲染可视区，数据量与帧率无关。

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
