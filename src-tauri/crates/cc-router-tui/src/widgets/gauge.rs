//! 限额进度条: 总览页 (健康度面板, 每行只放最紧的一条) 与订阅页 (详情面板, 每个设了上限的
//! 周期一条) 共用同一个 `LineGauge` 构造规则, 颜色走 [`crate::theme::Theme::quota_color`]。

use ratatui::style::Style;
use ratatui::widgets::LineGauge;

use crate::theme::Theme;

pub fn quota_gauge(ratio: f64, theme: &Theme) -> LineGauge<'static> {
    LineGauge::default()
        .ratio(ratio)
        .label("")
        .filled_symbol("━")
        .unfilled_symbol("─")
        .filled_style(Style::new().fg(theme.quota_color(ratio)))
        .unfilled_style(theme.border_style())
}
