# macOS Dock 图标动态切换 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** macOS 上窗口显示时 Dock 出现图标（Regular），窗口关进托盘后 Dock 图标消失（Accessory）。

**Architecture:** 删除启动期固定 `Accessory` 的设置，让 app 以默认 `Regular` 启动（启动时窗口本来就显示）；之后只有两个运行时切换点——`tray.rs::on_window_event` 关窗时降 `Accessory`，`tray.rs::reveal_window` 呼出时升 `Regular`。全部 `#[cfg(target_os = "macos")]`。

**Tech Stack:** Tauri 2.11（`AppHandle::set_activation_policy(&self, ActivationPolicy) -> tauri::Result<()>`，运行时版本，macOS-only，见 tauri-2.11.x `src/app.rs:640`）。

## Global Constraints

- 行为写死，不做设置项（用户已确认）。
- 所有平台相关代码用 `#[cfg(target_os = "macos")]`，非 macOS 平台零改动。
- policy 切换失败只 `warn!` 不中断（与托盘现有错误处理风格一致）。
- Rust 注释用英文/中文混合遵循文件现状（tray.rs 现状为中文注释）。
- activation policy 切换无法单测（需要真 macOS GUI 进程），验证靠 `cargo check` + `cargo test`（回归）+ `pnpm tauri dev` 手动 QA。
- Spec: `docs/superpowers/specs/2026-07-31-macos-dynamic-dock-icon-design.md`

---

### Task 1: tray.rs 两个切换点

**Files:**
- Modify: `src-tauri/src/tray.rs`（模块顶注、`reveal_window`、`on_window_event`）

**Interfaces:**
- Consumes: `tauri::AppHandle::set_activation_policy`（经 `Manager::app_handle()`，`Manager` 已在文件 import 里）
- Produces: 无新公开接口；`reveal_window` / `on_window_event` 签名不变（Task 2 依赖其行为，不依赖签名变化）

- [ ] **Step 1: 改 `reveal_window`——先升 Regular 再激活**

把 `src-tauri/src/tray.rs` 中 `reveal_window` 整个函数（含其文档注释，现约 152-164 行）替换为：

```rust
/// 把主窗口呼出到前台并抢键盘焦点。
///
/// 顺序很关键, 且 macOS 下 policy 必须排最先:
/// 1. 先把 activation policy 升回 Regular (Dock 图标出现)。tao 的 `set_focus`
///    走 `NSApp.activateIgnoringOtherApps`, Accessory 进程调用它常被
///    WindowServer 忽略 (Accessory 语义即「不参与前台激活」), 先升 Regular
///    才能保证抢到前台。
/// 2. 再 unminimize → show → set_focus: Tauri `WebviewWindow::set_focus` 在
///    macOS 下透传到 tao `Window::set_focus` (tao 0.35.x
///    src/platform_impl/macos/window.rs), 该实现仅在
///    `!is_minimized && is_visible` 时才会调用
///    `NSApp.activateIgnoringOtherApps(YES)`, 乱序会导致用户需要二次点击。
pub(crate) fn reveal_window<R: tauri::Runtime>(win: &tauri::WebviewWindow<R>) {
    #[cfg(target_os = "macos")]
    if let Err(e) = win
        .app_handle()
        .set_activation_policy(tauri::ActivationPolicy::Regular)
    {
        warn!(error = %e, "set_activation_policy(Regular) failed");
    }
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
}
```

- [ ] **Step 2: 改 `on_window_event`——关窗后降 Accessory**

把 `on_window_event` 中 `CloseRequested` 分支（现约 175-178 行）替换为：

```rust
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
        // macOS: 窗口收进托盘后降为 Accessory —— Dock 图标消失, 退出 Cmd+Tab。
        // 下次 reveal_window 会升回 Regular。
        #[cfg(target_os = "macos")]
        if let Err(e) = window
            .app_handle()
            .set_activation_policy(tauri::ActivationPolicy::Accessory)
        {
            warn!(error = %e, "set_activation_policy(Accessory) failed");
        }
    }
```

- [ ] **Step 3: 更新模块顶注**

模块顶注第 6-7 行（「macOS 上 cc-router 是纯菜单栏 app…Dock 里没有图标，托盘就是唯一常驻入口」）改为：

```rust
//! macOS 上 Dock 图标随窗口显隐动态切换（Regular ↔ Accessory，见 `reveal_window`
//! 与 `on_window_event`）：窗口收进托盘后 Dock 无图标，托盘是唯一常驻入口，
//! 所以菜单文案必须跟随用户选的界面语言。
```

- [ ] **Step 4: 编译 + 回归测试**

Run: `cd src-tauri && cargo check && cargo test tray`
Expected: check 通过；tray 的 5 个 locale 单测全部 PASS（本改动不触碰 locale 逻辑）。

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/tray.rs
git commit -m "feat(macos): 关窗降 Accessory、呼出升 Regular, Dock 图标随窗口显隐"
```

---

### Task 2: lib.rs 删除启动期 Accessory 固定

**Files:**
- Modify: `src-tauri/src/lib.rs`（`run()` 内 `build()` 与 `run()` 之间的 policy 段）

**Interfaces:**
- Consumes: 无（纯删除）
- Produces: app 启动即默认 `Regular`；`RunEvent::Reopen` → `tray::reveal_window` 链路保持不变

- [ ] **Step 1: 删除 Accessory 设置及配套注释**

删掉 `src-tauri/src/lib.rs` 中：
1. `run()` 开头的 `// 非 macOS 下不需要 mut (set_activation_policy 那段被 cfg 掉了)` 注释和 `#[allow(unused_mut)]`，`let mut app` 改回 `let app`（现约 53-55 行）；
2. `build()` 之后整段「macOS: 把 app 固定为 Accessory……」长注释 + `#[cfg(target_os = "macos")] app.set_activation_policy(tauri::ActivationPolicy::Accessory);`（现约 177-191 行），原地替换为一条短注释说明新机制：

```rust
    // macOS: 不再启动期固定 Accessory。app 以默认 Regular 启动 (启动时窗口
    // 本来就显示, Dock 该有图标); 之后 Dock 图标随窗口显隐动态切换, 见
    // tray.rs::reveal_window (升 Regular) 与 tray.rs::on_window_event (降 Accessory)。
    // Accessory 状态下 Spotlight / Launchpad / `open -a` 重开 app 会发
    // Reopen 事件, 由 on_run_event 唤回窗口 —— 该处理必须保留。
```

- [ ] **Step 2: 编译**

Run: `cd src-tauri && cargo check`
Expected: 通过，且无 `unused_mut` / unused import 警告。

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(macos): 启动改为默认 Regular, 移除启动期 Accessory 固定"
```

---

### Task 3: 文档同步（CLAUDE.md + README）

**Files:**
- Modify: `CLAUDE.md`（「macOS 是纯菜单栏 app（无 Dock 图标，不可配置）」条目）
- Modify: `README.md:119`（菜单栏 app 说明）

**Interfaces:**
- Consumes: Task 1/2 落地后的实际行为
- Produces: 文档与代码一致

- [ ] **Step 1: 重写 CLAUDE.md 条目**

把「**macOS 是纯菜单栏 app（无 Dock 图标，不可配置）**」整条替换为（保留 Reopen 必答题与 `Builder::run` 写法说明，删除已不存在的「build 后设 Accessory 零闪烁」论述）：

```markdown
- **macOS 的 Dock 图标随窗口显隐动态切换（写死，不可配置）**：app 以默认 `Regular` 启动（启动时窗口即显示）；`tray::on_window_event` 关窗时降 `Accessory`（Dock 图标消失、退出 Cmd+Tab），`tray::reveal_window` 呼出时先升 `Regular` 再 `unminimize → show → set_focus`——policy 必须排最先，Accessory 进程调 `activateIgnoringOtherApps` 常被 WindowServer 忽略。**由此带来的必答题**：Accessory 状态下 Spotlight / Launchpad / Finder / `open -a` 都会给已有进程发 `applicationShouldHandleReopen` → `RunEvent::Reopen`，必须在 `lib.rs::on_run_event` 里唤回主窗口，**不处理的话用户点了完全没反应**。注意 `Builder::run(ctx)` 不接受回调，所以入口写成 `.build(ctx).expect(...)` + `app.run(callback)`。最小化 / Cmd+H 不触发切换（窗口仍存在，Dock 图标保留），只有关闭按钮才收进托盘。
```

- [ ] **Step 2: 更新 README 第 119 行**

替换为：

```markdown
> **macOS 上 cc-router 的 Dock 图标随窗口显隐**：窗口打开时 Dock 有图标、可 Cmd+Tab 切换；关闭窗口后图标消失，入口只剩屏幕右上角的菜单栏。关闭窗口只是隐藏，代理会继续运行；要再打开窗口，点菜单栏图标、或在 Spotlight 里再搜一次 cc-router 即可。彻底退出请用菜单栏图标 →「退出 cc-router」。
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: 同步 macOS Dock 图标动态切换行为说明"
```

---

### Task 4: 手动 QA（真 macOS GUI 验证）

**Files:** 无代码改动。

- [ ] **Step 1: 启动 dev 版**

Run: `pnpm tauri dev`（仓库根目录）
Expected: 窗口显示，Dock 出现 cc-router 图标。

- [ ] **Step 2: 五条路径逐项验证**

1. 启动：窗口显示且 Dock 有图标 ✓/✗
2. 点红色关闭按钮：窗口隐藏、Dock 图标消失、托盘图标仍在 ✓/✗
3. 托盘左键 /「显示主窗口」：Dock 图标恢复、窗口前台并抢到键盘焦点（一次点击即可，无需二次点击）✓/✗
4. Dock 隐藏状态下用 Spotlight 打开 cc-router：同 3 ✓/✗
5. 窗口显示时 Cmd+Tab 列表里有 cc-router，且能切过去 ✓/✗

任何一条 ✗ → 停下按 superpowers:systematic-debugging 排查，不要继续。

- [ ] **Step 3: 更新项目 memory**

更新 `~/.claude/projects/-Users-finchxu-Documents-GitHub-cc-router/memory/architecture-decisions.md` 中纯菜单栏 app 的描述为动态切换语义（memory 不入 repo，无 commit）。
