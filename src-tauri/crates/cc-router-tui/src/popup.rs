//! 弹窗状态。`Popup::Help` 沿用 P1 的无状态弹窗; `Popup::Confirm` 是 P3b 新增的「y 是 / n 否」
//! 确认弹窗——打开时除 `Ctrl+C` 外的所有按键都归它, 具体语义见 `App::handle_key`。
//!
//! `Popup::Picker` (Task 3) 是过滤选择弹窗: 输入框 / 选中下标是它自己的状态, 打字 / 移动选中直接
//! 改 `PickerState` 自身, 不经过 `Action` 往返 (与 `Component::handle_key` 里方向键直接改页面自己
//! 的选中下标是同一套约定); 只有「选定」(⏎ → `PickerDone`) 与「取消」(Esc → `ClosePopup`) 两件事
//! 才产出 `Action`。以后再加新弹窗变体, 只需在这里多加一个变体、`App` 里按变体补一个 match 分
//! 支——`App` 已经是穷尽 match, 编译器会逼着改全。

use crate::action::Action;
use crate::widgets::picker::PickerState;

#[derive(Debug, Clone, PartialEq)]
pub enum Popup {
    Help,
    Confirm(ConfirmState),
    Picker(PickerState),
}

/// 一次「是 / 否」确认: `prompt` 是正文, `on_yes` 是用户选「是」时真正要执行的 `Action`——由
/// `Action::Confirmed` 触发, 先让当前页面丢弃草稿 (`discard_changes`), 再按普通 `update` 路径
/// 执行 (此时 dirty 已清空, 不会被再次拦截确认)。
#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmState {
    pub prompt: String,
    pub on_yes: Box<Action>,
}
