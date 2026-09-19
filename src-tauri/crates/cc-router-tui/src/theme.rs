//! 语义色表。页面只问「healthy 是什么颜色」, 不直接写 tailwind 色号。
//!
//! 三条规则 (spec §4.5): 只设前景色, 不填背景 (终端背景深浅未知); `COLORTERM` 不是 truecolor 就退回
//! ANSI 16 色的同语义映射; 设了 `NO_COLOR` 就完全不输出颜色, 状态改靠符号区分。

use ratatui::style::palette::tailwind;
use ratatui::style::{Color, Modifier, Style};

use crate::client::dto::SubscriptionState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Ansi16,
    NoColor,
}

impl ColorMode {
    /// `env` 注入是为了可测; 生产传 `|k| std::env::var(k).ok()`。
    pub fn detect(env: impl Fn(&str) -> Option<String>) -> Self {
        // no-color.org: 变量存在且非空即生效, 不看具体值。
        if env("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Self::NoColor;
        }
        match env("COLORTERM").as_deref() {
            Some("truecolor") | Some("24bit") => Self::TrueColor,
            _ => Self::Ansi16,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub mode: ColorMode,
    /// 品牌强调色: 当前标签、logo、键位。
    pub accent: Color,
    /// 次要文字: 标签名、单位、占位。
    pub muted: Color,
    pub border: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    healthy: Color,
    rate_limited: Color,
    quota_exhausted: Color,
    transient_error: Color,
    auth_failed: Color,
    disabled: Color,
}

impl Theme {
    pub fn new(mode: ColorMode) -> Self {
        match mode {
            ColorMode::TrueColor => Self {
                mode,
                accent: tailwind::ORANGE.c400,
                muted: tailwind::SLATE.c400,
                border: tailwind::SLATE.c600,
                ok: tailwind::EMERALD.c400,
                warn: tailwind::AMBER.c400,
                err: tailwind::RED.c400,
                healthy: tailwind::EMERALD.c400,
                rate_limited: tailwind::AMBER.c400,
                quota_exhausted: tailwind::VIOLET.c400,
                transient_error: tailwind::ORANGE.c400,
                auth_failed: tailwind::RED.c400,
                disabled: tailwind::SLATE.c500,
            },
            ColorMode::Ansi16 => Self {
                mode,
                accent: Color::Yellow,
                muted: Color::DarkGray,
                border: Color::DarkGray,
                ok: Color::Green,
                warn: Color::Yellow,
                err: Color::Red,
                healthy: Color::Green,
                rate_limited: Color::Yellow,
                quota_exhausted: Color::Magenta,
                transient_error: Color::LightRed,
                auth_failed: Color::Red,
                disabled: Color::DarkGray,
            },
            ColorMode::NoColor => Self {
                mode,
                accent: Color::Reset,
                muted: Color::Reset,
                border: Color::Reset,
                ok: Color::Reset,
                warn: Color::Reset,
                err: Color::Reset,
                healthy: Color::Reset,
                rate_limited: Color::Reset,
                quota_exhausted: Color::Reset,
                transient_error: Color::Reset,
                auth_failed: Color::Reset,
                disabled: Color::Reset,
            },
        }
    }

    pub fn state_color(&self, state: SubscriptionState) -> Color {
        match state {
            SubscriptionState::Healthy => self.healthy,
            SubscriptionState::RateLimited => self.rate_limited,
            SubscriptionState::QuotaExhausted => self.quota_exhausted,
            SubscriptionState::TransientError => self.transient_error,
            SubscriptionState::AuthFailed => self.auth_failed,
            SubscriptionState::Disabled | SubscriptionState::Unknown => self.disabled,
        }
    }

    /// 限额进度条颜色: ≥100% 满 `err`、≥80% 告急 `warn`、否则 `ok`。总览页、订阅页共用同一判定。
    pub fn quota_color(&self, ratio: f64) -> Color {
        if ratio >= 1.0 {
            self.err
        } else if ratio >= 0.8 {
            self.warn
        } else {
            self.ok
        }
    }

    pub fn accent_bold(&self) -> Style {
        Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
    }

    pub fn muted_style(&self) -> Style {
        Style::new().fg(self.muted)
    }

    pub fn border_style(&self) -> Style {
        Style::new().fg(self.border)
    }

    /// 渐变动效依赖 RGB 插值, 16 色下会跳变, 无色时没有意义 (spec §6.4)。
    pub fn supports_fx(&self) -> bool {
        self.mode == ColorMode::TrueColor
    }
}

/// 状态符号。`NO_COLOR` 下这是区分状态的唯一手段, 所以六个状态的符号两两不同。
pub fn state_symbol(state: SubscriptionState) -> &'static str {
    match state {
        SubscriptionState::Healthy => "●",
        SubscriptionState::RateLimited => "◐",
        SubscriptionState::QuotaExhausted => "◑",
        SubscriptionState::TransientError => "◒",
        SubscriptionState::AuthFailed => "✕",
        SubscriptionState::Disabled => "○",
        SubscriptionState::Unknown => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn detects_color_mode() {
        assert_eq!(ColorMode::detect(env(&[("COLORTERM", "truecolor")])), ColorMode::TrueColor);
        assert_eq!(ColorMode::detect(env(&[("COLORTERM", "24bit")])), ColorMode::TrueColor);
        assert_eq!(ColorMode::detect(env(&[])), ColorMode::Ansi16);
        assert_eq!(ColorMode::detect(env(&[("COLORTERM", "yes")])), ColorMode::Ansi16);
    }

    #[test]
    fn no_color_wins_over_colorterm_but_empty_value_does_not_count() {
        assert_eq!(ColorMode::detect(env(&[("NO_COLOR", "1"), ("COLORTERM", "truecolor")])), ColorMode::NoColor);
        assert_eq!(ColorMode::detect(env(&[("NO_COLOR", ""), ("COLORTERM", "truecolor")])), ColorMode::TrueColor);
    }

    #[test]
    fn no_color_theme_emits_no_colors() {
        let t = Theme::new(ColorMode::NoColor);
        assert_eq!(t.accent, Color::Reset);
        assert_eq!(t.state_color(SubscriptionState::AuthFailed), Color::Reset);
        assert!(!t.supports_fx());
    }

    #[test]
    fn state_symbols_are_pairwise_distinct() {
        use SubscriptionState::*;
        let all = [Healthy, RateLimited, QuotaExhausted, TransientError, AuthFailed, Disabled, Unknown];
        let mut seen: Vec<&str> = all.iter().map(|s| state_symbol(*s)).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), all.len());
    }
}
