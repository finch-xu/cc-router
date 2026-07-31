# macOS Dock 图标随窗口显示/隐藏动态切换 — 设计

日期：2026-07-31
状态：已获用户批准

## 背景与目标

当前 cc-router 在 macOS 上是纯菜单栏 app：启动时把 activation policy 固定为
`Accessory`（`lib.rs::run`，`build()` 与 `run()` 之间），Dock 永远没有图标。
用户反馈「经常找不到 app 窗口」，希望：

- 窗口显示时 → Dock 有图标（`Regular`，同时进 Cmd+Tab）
- 窗口关闭（收进托盘）后 → Dock 图标消失（`Accessory`）

行为**写死**，不做设置项（用户已确认）。

## 方案

启动默认 `Regular` + 两个运行时切换点。改动 2 个文件 3 个点，全部
`#[cfg(target_os = "macos")]`：

1. **`src-tauri/src/lib.rs`**：删除 `build()` 之后的
   `app.set_activation_policy(Accessory)` 及配套长注释。app 以 macOS 默认的
   `Regular` 启动；启动时窗口本来就会显示（`window::apply_startup_geometry`
   里 `show()`），语义自洽，无闪烁问题（闪烁只发生在「启动时要隐藏」场景）。
   `RunEvent::Reopen` 处理保留——Accessory 状态下 Spotlight / Launchpad /
   `open -a` 重开仍靠它唤回窗口。

2. **`src-tauri/src/tray.rs::on_window_event`**：`CloseRequested` 分支在
   `hide()` 之后调 `window.app_handle().set_activation_policy(Accessory)`
   （Tauri 2 `AppHandle` 运行时版本，返回 `Result`，失败只 warn 不中断）。

3. **`src-tauri/src/tray.rs::reveal_window`**：在现有
   `unminimize → show → set_focus` 序列**之前**先切 `Regular`。顺序理由：
   tao 的 `set_focus` 依赖 `NSApp.activateIgnoringOtherApps`，Accessory 进程
   调用它常被 WindowServer 忽略（Accessory 语义即「不参与前台激活」），必须
   先升 Regular 再激活。函数顺序注释同步更新。

## 行为边界（有意为之）

- 最小化（黄按钮）/ Cmd+H：窗口仍「存在」，Dock 图标保留；只有红色关闭按钮
  才收进托盘并隐藏 Dock 图标。
- Regular 模式下 app 进 Cmd+Tab、Dock 图标可右键 Quit——policy 绑定的标准
  mac 行为，属预期。
- Dock 图标点击唤回窗口走已有 `Reopen` 处理。

## 文档同步

- `CLAUDE.md`「macOS 是纯菜单栏 app（无 Dock 图标，不可配置）」条目重写为新语义。
- `tray.rs` 模块顶注（「Dock 里没有图标，托盘就是唯一常驻入口」）修正。
- README 若提及纯菜单栏一并修正。
- 项目 memory `architecture-decisions` 同步。

## 测试策略

activation policy 切换需要真 macOS GUI 进程，不写单测。验证：
`cargo check` + `pnpm tauri dev` 手动 QA 五条路径：

1. 启动：窗口显示且 Dock 有图标
2. 红色关闭按钮：窗口隐藏、Dock 图标消失、托盘仍在
3. 托盘「显示主窗口」/ 左键：Dock 图标恢复、窗口前台抢焦点
4. Dock 隐藏状态下 Spotlight 重开：同上
5. 窗口显示时 Cmd+Tab 能切到 cc-router

现有 `tray.rs` locale 单测不受影响。
