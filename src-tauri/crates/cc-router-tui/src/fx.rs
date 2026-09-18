//! 动效 token。页面只调这里的语义函数, 时长 / 缓动不在别处散写 (CLAUDE.md「工作风格偏好」)。
//!
//! tachyonfx 的效果是对**已经画好的缓冲区**做后处理: 内容当帧就绪、按键当帧生效, 效果只是叠在上面的一层;
//! 同一个 [`FxKey`] 再次添加会取消旧效果。所以这里的每个效果都满足「不阻塞输入、可被打断」。
//!
//! **只动前景色**: 我们不知道用户终端的背景色 (theme.rs 同一条规则), 所以不用 `sweep_in` / `slide_in` /
//! `fade_from` 这类需要指定背景色的效果; 方向感由 `fade_from_fg` + `SweepPattern` 给出。
//! 循环效果一个都没有 —— 「重连中」用 250ms tick 驱动的 throbber, 否则 app 没开的几个小时里 TUI 会一直跑 60fps。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use tachyonfx::pattern::SweepPattern;
use tachyonfx::{fx, Duration, Effect, EffectManager, Interpolation};

/// 标称时长 (毫秒)。入场用 `QuadOut`, 退场用 `QuadIn`。
pub mod ms {
    pub const STARTUP: u32 = 400;
    pub const PAGE: u32 = 150;
    pub const POPUP_OPEN: u32 = 120;
    pub const POPUP_CLOSE: u32 = 100;
    pub const TOAST_IN: u32 = 200;
    pub const TOAST_OUT: u32 = 300;
    pub const ROW_CHANGED: u32 = 600;
    pub const VALUE_CHANGED: u32 = 400;
}

// `Default` 只是为了满足 `EffectManager<K>: Default` 的派生约束 (tachyonfx 0.25), 没有语义。
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum FxKey {
    #[default]
    Startup,
    Page,
    Popup,
    Toast,
    /// 订阅 id
    Row(String),
    Value(&'static str),
}

/// 切页方向: 往右边的标签走是 `Forward`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Forward,
    Backward,
}

pub struct Fx {
    enabled: bool,
    mgr: EffectManager<FxKey>,
}

impl Fx {
    /// `enabled = false` 时所有方法都是空操作, 调用点不需要分支。
    pub fn new(enabled: bool) -> Self {
        Self { enabled, mgr: EffectManager::default() }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 有效果在播 → 主循环切到约 60fps; 播完回落到按需重绘。
    pub fn is_running(&self) -> bool {
        self.mgr.is_running()
    }

    /// 每帧所有 widget 画完之后调用一次。
    pub fn process(&mut self, elapsed: std::time::Duration, buf: &mut Buffer, area: Rect) {
        if self.enabled {
            self.mgr.process_effects(Duration::from_millis(elapsed.as_millis() as u32), buf, area);
        }
    }

    fn add(&mut self, key: FxKey, effect: Effect) {
        if self.enabled {
            self.mgr.add_unique_effect(key, effect);
        }
    }

    /// 启动: logo 逐格凝聚, 其余内容从暗色淡入。
    pub fn startup(&mut self, logo: Rect, screen: Rect, from: Color) {
        let logo_fx = fx::coalesce((ms::STARTUP, Interpolation::QuadOut)).with_area(logo);
        let rest_fx = fx::fade_from_fg(from, (ms::STARTUP * 3 / 4, Interpolation::QuadOut)).with_area(screen);
        self.add(FxKey::Startup, fx::parallel(&[rest_fx, logo_fx]));
    }

    /// 切页: 内容区沿标签移动的方向扫入。
    pub fn page_enter(&mut self, dir: Dir, content: Rect, from: Color) {
        let pattern = match dir {
            Dir::Forward => SweepPattern::left_to_right(12),
            Dir::Backward => SweepPattern::right_to_left(12),
        };
        let effect = fx::fade_from_fg(from, (ms::PAGE, Interpolation::QuadOut)).with_pattern(pattern).with_area(content);
        self.add(FxKey::Page, effect);
    }

    pub fn popup_open(&mut self, popup: Rect, from: Color) {
        self.add(FxKey::Popup, fx::fade_from_fg(from, (ms::POPUP_OPEN, Interpolation::QuadOut)).with_area(popup));
    }

    /// 弹窗已经不画了, 它原来盖住的那块内容重新凝聚出来 —— 不需要让弹窗多活 100ms。
    pub fn popup_close(&mut self, popup: Rect) {
        self.add(FxKey::Popup, fx::coalesce((ms::POPUP_CLOSE, Interpolation::QuadIn)).with_area(popup));
    }

    pub fn toast_in(&mut self, toast: Rect, from: Color) {
        let effect = fx::fade_from_fg(from, (ms::TOAST_IN, Interpolation::QuadOut))
            .with_pattern(SweepPattern::right_to_left(8))
            .with_area(toast);
        self.add(FxKey::Toast, effect);
    }

    pub fn toast_out(&mut self, toast: Rect) {
        self.add(FxKey::Toast, fx::dissolve((ms::TOAST_OUT, Interpolation::QuadIn)).with_area(toast));
    }

    /// 功能性动画: 某条订阅的状态变了, 那一行从状态色回落, 视线直接落过去。
    pub fn row_changed(&mut self, id: &str, row: Rect, color: Color) {
        let effect = fx::fade_from_fg(color, (ms::ROW_CHANGED, Interpolation::QuadOut)).with_area(row);
        self.add(FxKey::Row(id.to_string()), effect);
    }

    pub fn value_changed(&mut self, key: &'static str, cell: Rect, color: Color) {
        let effect = fx::fade_from_fg(color, (ms::VALUE_CHANGED, Interpolation::QuadOut)).with_area(cell);
        self.add(FxKey::Value(key), effect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration as StdDuration;

    const AREA: Rect = Rect::new(0, 0, 40, 10);
    const PART: Rect = Rect::new(2, 2, 20, 3);

    type Trigger = Box<dyn Fn(&mut Fx)>;

    /// 每个语义效果的 (名字, 标称时长, 触发方式)。新增效果时加进来, 终止性测试会自动覆盖它。
    fn all() -> Vec<(&'static str, u32, Trigger)> {
        vec![
            ("startup", ms::STARTUP, Box::new(|f| f.startup(PART, AREA, Color::DarkGray))),
            ("page_fwd", ms::PAGE, Box::new(|f| f.page_enter(Dir::Forward, AREA, Color::DarkGray))),
            ("page_back", ms::PAGE, Box::new(|f| f.page_enter(Dir::Backward, AREA, Color::DarkGray))),
            ("popup_open", ms::POPUP_OPEN, Box::new(|f| f.popup_open(PART, Color::DarkGray))),
            ("popup_close", ms::POPUP_CLOSE, Box::new(|f| f.popup_close(PART))),
            ("toast_in", ms::TOAST_IN, Box::new(|f| f.toast_in(PART, Color::DarkGray))),
            ("toast_out", ms::TOAST_OUT, Box::new(|f| f.toast_out(PART))),
            ("row_changed", ms::ROW_CHANGED, Box::new(|f| f.row_changed("id", PART, Color::Red))),
            ("value_changed", ms::VALUE_CHANGED, Box::new(|f| f.value_changed("requests", PART, Color::Red))),
        ]
    }

    /// 效果永不结束会把渲染锁死在 60fps —— 每个效果推进标称时长后必须停。
    #[test]
    fn every_effect_terminates_after_its_nominal_duration() {
        for (name, nominal, trigger) in all() {
            let mut fx = Fx::new(true);
            let mut buf = Buffer::empty(AREA);
            trigger(&mut fx);
            assert!(fx.is_running(), "{name}: 触发后应该在播");
            fx.process(StdDuration::from_millis(u64::from(nominal) / 2), &mut buf, AREA);
            assert!(fx.is_running(), "{name}: 半程不应该结束");
            fx.process(StdDuration::from_millis(u64::from(nominal)), &mut buf, AREA);
            assert!(!fx.is_running(), "{name}: 超过标称时长 {nominal}ms 仍在播");
        }
    }

    #[test]
    fn disabled_fx_never_runs() {
        for (name, _, trigger) in all() {
            let mut fx = Fx::new(false);
            trigger(&mut fx);
            assert!(!fx.is_running(), "{name}");
        }
    }

    /// 连续切页: 新效果取消旧效果, 不会越叠越多。
    #[test]
    fn same_key_replaces_the_previous_effect() {
        let mut fx = Fx::new(true);
        let mut buf = Buffer::empty(AREA);
        for _ in 0..50 {
            fx.page_enter(Dir::Forward, AREA, Color::DarkGray);
            fx.process(StdDuration::from_millis(10), &mut buf, AREA);
        }
        fx.process(StdDuration::from_millis(u64::from(ms::PAGE)), &mut buf, AREA);
        assert!(!fx.is_running());
    }

    #[test]
    fn rows_with_different_ids_run_side_by_side() {
        let mut fx = Fx::new(true);
        let mut buf = Buffer::empty(AREA);
        fx.row_changed("a", PART, Color::Red);
        fx.process(StdDuration::from_millis(u64::from(ms::ROW_CHANGED) - 100), &mut buf, AREA);
        fx.row_changed("b", PART, Color::Red);
        fx.process(StdDuration::from_millis(200), &mut buf, AREA);
        assert!(fx.is_running(), "b 才播了 200ms");
    }
}
