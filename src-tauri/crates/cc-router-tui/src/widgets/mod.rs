//! 跨页面复用的小部件。

pub mod badge;
pub mod confirm;
pub mod gauge;
pub mod help;
pub mod keybar;
pub mod picker;
pub mod toast;

/// 第 `tick` 帧的 spinner 状态。`calc_step(0)` 在 throbber-widgets-tui 里的含义是「随机取一格」,
/// 所以步长永远不传 0 —— 否则同一状态画两次会得到不同的帧。
pub fn spinner_state(tick: u64) -> throbber_widgets_tui::ThrobberState {
    let mut state = throbber_widgets_tui::ThrobberState::default();
    state.calc_step((tick % 120) as i8 + 1);
    state
}
