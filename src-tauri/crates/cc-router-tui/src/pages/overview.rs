//! 总览页: logo + 代理地址、今日三个数字、按小时请求量、订阅健康度。

use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, LineGauge, Padding, Paragraph, Sparkline};
use ratatui::Frame;
use throbber_widgets_tui::{Throbber, BRAILLE_SIX};
use tui_big_text::{BigText, PixelSize};

use super::{Component, DrawCtx};
use crate::action::{Action, Cmd, OverviewData};
use crate::client::dto::{hourly_buckets, OverallStats, ProxyStatus, Subscription};
use crate::format::{compact, fit, mmss, percent, thousands};
use crate::i18n::Strings;
use crate::widgets::badge::{badge, severity};
use crate::widgets::keybar::Hint;
use crate::widgets::spinner_state;

const LOGO_TEXT: &str = "cc-router";
/// Quadrant 像素: 8×8 字模横竖各减半 → 每字 4 列 × 4 行。
const LOGO_WIDTH: u16 = LOGO_TEXT.len() as u16 * 4;
const LOGO_HEIGHT: u16 = 4;
const TODAY_WIDTH: u16 = 23;
const MID_HEIGHT: u16 = 5;
/// 健康度面板至少留 5 行 (边框 2 + 3 条订阅), 不够就先让 logo 让位 (spec §5.1)。
const MIN_HEALTH_HEIGHT: u16 = 5;

/// 健康度一行的定宽列 (显示列数)。状态列要放得下「限流 · 00:42」; P6 加 en / ja 后要按最长译文重新量。
const STATUS_COL: usize = 22;
const QUOTA_LABEL_COL: usize = 8;
const PERCENT_COL: usize = 5;

const SSE_REFETCH: [&str; 2] = ["subscription_state_changed", "subscription_quota_reached"];

#[derive(Default)]
pub struct Overview {
    status: Option<ProxyStatus>,
    auth_enabled: bool,
    stats: Option<OverallStats>,
    hourly: [u64; 24],
    subscriptions: Vec<Subscription>,
    loaded: bool,
    /// 下一帧要闪一下的订阅 id / 数字; `draw` 取走。
    flash_rows: Vec<String>,
    flash_values: Vec<&'static str>,
    /// 上一帧 logo 画在哪 —— 启动动效要用。
    logo_area: Option<Rect>,
}

impl Overview {
    pub fn logo_area(&self) -> Option<Rect> {
        self.logo_area
    }

    fn set_stats(&mut self, stats: OverallStats) {
        if let Some(old) = &self.stats {
            if old.total_requests != stats.total_requests {
                self.flash_values.push("requests");
            }
            if old.success_rate_pct != stats.success_rate_pct {
                self.flash_values.push("success");
            }
            if old.total_tokens() != stats.total_tokens() {
                self.flash_values.push("tokens");
            }
        }
        self.stats = Some(stats);
    }

    fn set_subscriptions(&mut self, mut subs: Vec<Subscription>) {
        if self.loaded {
            for new in &subs {
                let changed = self.subscriptions.iter().find(|old| old.id == new.id).is_some_and(|old| {
                    (old.state, old.enabled, old.is_dispatchable) != (new.state, new.enabled, new.is_dispatchable)
                });
                if changed {
                    self.flash_rows.push(new.id.clone());
                }
            }
        }
        // 稳定排序: 同一档内保持后端给的顺序 (与桌面端一致)。
        subs.sort_by_key(severity);
        self.subscriptions = subs;
    }

    fn apply(&mut self, data: OverviewData) {
        self.status = Some(data.status);
        self.auth_enabled = data.settings.auth_enabled;
        self.set_stats(data.stats);
        self.hourly = hourly_buckets(&data.series);
        self.set_subscriptions(data.subscriptions);
        self.loaded = true;
    }

    fn draw_hero(&mut self, frame: &mut Frame, area: Rect, show_logo: bool, ctx: &DrawCtx) {
        let info_area = if show_logo {
            // 左边空 2 列, 让 logo 与下面面板里的文字左对齐 (边框 1 + 内距 1); logo 与信息区之间留 3 列。
            // 信息区不再额外收内距 (fix round 2): 80 列终端下旧的「2 + 36 + 2 + 内距 2」只剩 36 列,
            // 放不下「监听 0.0.0.0」这行 (http 37 列 / https 38 列), 现在满打满算给到 39 列。
            let [_, logo, _, info] = Layout::horizontal([
                Constraint::Length(2),
                Constraint::Length(LOGO_WIDTH),
                Constraint::Length(3),
                Constraint::Min(0),
            ])
            .areas(area);
            let big = BigText::builder()
                .pixel_size(PixelSize::Quadrant)
                .style(Style::new().fg(ctx.theme.accent))
                .lines(vec![LOGO_TEXT.into()])
                .build();
            frame.render_widget(big, logo);
            self.logo_area = Some(logo);
            info.centered_vertically(Constraint::Length(2))
        } else {
            self.logo_area = None;
            // 没有 logo 时手动留 2 列左内距, 让文字仍然与下面面板里的内容左对齐。
            area.inner(ratatui::layout::Margin::new(2, 0))
        };

        let s = ctx.s;
        let mut lines = Vec::new();
        if let Some(status) = &self.status {
            let mut spans = vec![Span::styled(status.base_url.clone(), ctx.theme.accent_bold())];
            if status.listen_all {
                spans.push(Span::styled(format!(" · {}", s.ov_listen_all), Style::new().fg(ctx.theme.warn)));
            }
            lines.push(Line::from(spans));
            let ok = self.subscriptions.iter().filter(|x| x.is_dispatchable).count();
            let auth = if self.auth_enabled { s.ov_auth_on } else { s.ov_auth_off };
            lines.push(Line::styled(
                format!("{auth} · {}", (s.ov_subs_summary)(self.subscriptions.len(), ok)),
                ctx.theme.muted_style(),
            ));
        }
        frame.render_widget(Paragraph::new(lines), info_area);
    }

    fn draw_today(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx, flash_values: &[&'static str]) {
        let s = ctx.s;
        let block = panel(ctx, s.ov_today);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let (requests, success, tokens) = match &self.stats {
            Some(st) if st.total_requests > 0 => {
                (thousands(st.total_requests), percent(st.success_rate_pct), compact(st.total_tokens()))
            }
            Some(_) => ("0".into(), "—".into(), "0".into()),
            None => ("…".into(), "…".into(), "…".into()),
        };
        let rows = [("requests", s.ov_requests, requests), ("success", s.ov_success_rate, success), ("tokens", s.ov_tokens, tokens)];
        for (i, (key, label, value)) in rows.into_iter().enumerate() {
            let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1).intersection(inner);
            let [l, v] = Layout::horizontal([Constraint::Min(0), Constraint::Length(9)]).areas(row);
            frame.render_widget(Line::styled(label, ctx.theme.muted_style()), l);
            frame.render_widget(Line::raw(value).right_aligned(), v);
            if flash_values.contains(&key) {
                ctx.fx.value_changed(key, v, ctx.theme.accent);
            }
        }
    }

    fn draw_hourly(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
        let block = panel(ctx, ctx.s.ov_hourly);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let [bars, axis] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);

        // 每小时占 1–4 列: 24 根 1 列宽的柱子在 56 列宽的面板里显得太挤。
        let per_hour = (inner.width / 24).clamp(1, 4) as usize;
        let data: Vec<u64> = self.hourly.iter().flat_map(|v| std::iter::repeat_n(*v, per_hour)).collect();
        frame.render_widget(Sparkline::default().data(&data).style(Style::new().fg(ctx.theme.accent)), bars);

        let mut line = String::new();
        for h in [0usize, 6, 12, 18] {
            let col = h * per_hour;
            if col >= line.len() {
                line.push_str(&" ".repeat(col - line.len()));
                line.push_str(&format!("{h}h"));
            }
        }
        frame.render_widget(Line::styled(line, ctx.theme.muted_style()), axis);
    }

    fn draw_health(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx, flash_rows: &[String]) {
        let s = ctx.s;
        let block = panel(ctx, s.ov_health)
            .title_bottom(Line::from(format!(" {} ", self.subscriptions.len())).right_aligned().style(ctx.theme.muted_style()));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if !self.loaded {
            let mut state = spinner_state(ctx.tick);
            let throbber = Throbber::default().label(s.loading).throbber_set(BRAILLE_SIX).style(ctx.theme.muted_style());
            frame.render_stateful_widget(throbber, inner, &mut state);
            return;
        }
        if self.subscriptions.is_empty() {
            frame.render_widget(Line::styled(s.ov_no_subs, ctx.theme.muted_style()), inner);
            return;
        }

        let capacity = inner.height as usize;
        let overflow = self.subscriptions.len() > capacity;
        let shown = if overflow { capacity.saturating_sub(1) } else { self.subscriptions.len() };
        // 窄终端名字列 18, 宽终端多给一些; 其余列定宽, 进度条吃掉剩下的。
        let name_col: usize = if inner.width >= 110 { 28 } else { 18 };

        for (i, sub) in self.subscriptions.iter().take(shown).enumerate() {
            let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            let b = badge(sub, ctx.theme, s);
            let status = match sub.cooldown_until.filter(|until| *until > ctx.now_ms && sub.enabled) {
                Some(until) => format!("{} · {}", b.label, mmss(until - ctx.now_ms)),
                None => b.label.to_string(),
            };
            let left_width = (2 + name_col + 2 + STATUS_COL + 2) as u16;
            let [left, label, gauge, pct] = Layout::horizontal([
                Constraint::Length(left_width),
                Constraint::Length(QUOTA_LABEL_COL as u16 + 1),
                Constraint::Min(0),
                Constraint::Length(PERCENT_COL as u16 + 1),
            ])
            .areas(row);

            frame.render_widget(
                Line::from(vec![
                    Span::styled(format!("{} ", b.symbol), Style::new().fg(b.color)),
                    Span::raw(fit(&sub.display_name, name_col)),
                    Span::raw("  "),
                    Span::styled(fit(&status, STATUS_COL), Style::new().fg(b.color)),
                ]),
                left,
            );
            match sub.tightest_quota().and_then(|q| q.ratio().map(|r| (q, r))) {
                Some((q, ratio)) => {
                    let color = if ratio >= 1.0 {
                        ctx.theme.err
                    } else if ratio >= 0.8 {
                        ctx.theme.warn
                    } else {
                        ctx.theme.ok
                    };
                    frame.render_widget(Line::styled(s.quota_period(q.period), ctx.theme.muted_style()), label);
                    frame.render_widget(
                        LineGauge::default()
                            .ratio(ratio)
                            .label("")
                            .filled_symbol("━")
                            .unfilled_symbol("─")
                            .filled_style(Style::new().fg(color))
                            .unfilled_style(ctx.theme.border_style()),
                        gauge,
                    );
                    frame.render_widget(Line::raw(format!("{:.0}%", ratio * 100.0)).right_aligned(), pct);
                }
                None => frame.render_widget(Line::styled("—", ctx.theme.muted_style()), label),
            }

            if flash_rows.contains(&sub.id) {
                ctx.fx.row_changed(&sub.id, row, b.color);
            }
        }
        if overflow && capacity > 0 {
            let row = Rect::new(inner.x, inner.y + shown as u16, inner.width, 1);
            let hidden = self.subscriptions.len() - shown;
            frame.render_widget(Line::styled((s.ov_more_rows)(hidden), ctx.theme.muted_style()), row);
        }
    }
}

fn panel<'a>(ctx: &DrawCtx, title: &str) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(ctx.theme.border_style())
        .title_top(format!(" {title} "))
        .padding(Padding::horizontal(1))
}

impl Component for Overview {
    fn handle_key(&mut self, _key: KeyEvent) -> Option<Action> {
        None
    }

    fn update(&mut self, action: &Action) -> Vec<Cmd> {
        match action {
            Action::Refresh | Action::Connected { .. } => vec![Cmd::FetchOverview],
            Action::Sse { name, .. } if SSE_REFETCH.contains(&name.as_str()) => vec![Cmd::FetchSubscriptions],
            Action::OverviewLoaded(data) => {
                self.apply((**data).clone());
                Vec::new()
            }
            Action::SubscriptionsLoaded(subs) => {
                self.set_subscriptions(subs.clone());
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx) {
        // 取走待播的闪烁队列: 不管下面两个面板是不是提前 return, self.flash_rows / self.flash_values
        // 从这一帧起都是空的, 不会残留到以后的帧 (F3)。
        let flash_rows = std::mem::take(&mut self.flash_rows);
        let flash_values = std::mem::take(&mut self.flash_values);

        let show_logo = area.height >= LOGO_HEIGHT + MID_HEIGHT + MIN_HEALTH_HEIGHT;
        let hero_height = if show_logo { LOGO_HEIGHT } else { 2 };
        let [hero, mid, health] =
            Layout::vertical([Constraint::Length(hero_height), Constraint::Length(MID_HEIGHT), Constraint::Min(0)]).areas(area);
        let [today, hourly] = Layout::horizontal([Constraint::Length(TODAY_WIDTH), Constraint::Min(0)]).areas(mid);

        self.draw_hero(frame, hero, show_logo, ctx);
        self.draw_today(frame, today, ctx, &flash_values);
        self.draw_hourly(frame, hourly, ctx);
        self.draw_health(frame, health, ctx, &flash_rows);
    }

    fn hints(&self, s: &'static Strings) -> Vec<Hint<'static>> {
        vec![("r", s.key_refresh)]
    }
}
