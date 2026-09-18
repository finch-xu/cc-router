//! 界面文案。`struct Strings` + 每种语言一个 `const`: 加字段时漏填任何一种语言都是编译错误,
//! 不需要运行时的「缺 key」检查。带参数的文案用 `fn` 指针, 各语言自己决定语序。
//!
//! 文案与桌面端独立一份 (TUI 用语更短), 但状态名等术语沿用桌面端 `src/i18n/locales/zh.json` 的叫法。
//! **en / ja 译文在 P6 补**: 现在 [`strings`] 对三种语言都返回 [`ZH`], 语言解析逻辑已经是最终形态。

use crate::client::dto::{QuotaPeriod, SubscriptionState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
    Ja,
}

impl Lang {
    /// `preferred` 来自桌面端设置 (`"system"` / `"zh"` / `"en"` / `"ja"`)。
    /// `"system"` 时按 `LC_ALL` → `LC_MESSAGES` → `LANG` 取系统语言, 映射规则与桌面端
    /// `src/i18n/index.tsx::detectSystemLocale` / `tray.rs` 一致: `zh*` → zh, `ja*` → ja, 其余 → en。
    pub fn resolve(preferred: &str, env: impl Fn(&str) -> Option<String>) -> Self {
        let tag = match preferred {
            "system" | "" => ["LC_ALL", "LC_MESSAGES", "LANG"]
                .iter()
                .find_map(|k| env(k).filter(|v| !v.is_empty()))
                .unwrap_or_default(),
            other => other.to_string(),
        };
        let lower = tag.to_lowercase();
        if lower.starts_with("zh") {
            Self::Zh
        } else if lower.starts_with("ja") {
            Self::Ja
        } else {
            Self::En
        }
    }
}

pub struct Strings {
    /// 五个标签, 顺序即 `1`–`5`。
    pub tabs: [&'static str; 5],
    pub conn_connecting: &'static str,
    pub conn_connected: &'static str,
    pub conn_reconnecting: &'static str,

    pub key_switch_tab: &'static str,
    pub key_refresh: &'static str,
    pub key_help: &'static str,
    pub key_quit: &'static str,
    pub key_close: &'static str,

    pub help_title: &'static str,
    /// (键, 说明)
    pub help_rows: &'static [(&'static str, &'static str)],

    pub too_small: &'static str,
    pub coming_soon: &'static str,
    pub loading: &'static str,
    pub version_mismatch: fn(tui: &str, app: &str) -> String,

    pub ov_today: &'static str,
    pub ov_requests: &'static str,
    pub ov_success_rate: &'static str,
    pub ov_tokens: &'static str,
    pub ov_hourly: &'static str,
    pub ov_health: &'static str,
    pub ov_auth_on: &'static str,
    pub ov_auth_off: &'static str,
    pub ov_listen_all: &'static str,
    pub ov_subs_summary: fn(total: usize, dispatchable: usize) -> String,
    pub ov_no_subs: &'static str,
    pub ov_more_rows: fn(hidden: usize) -> String,

    pub st_healthy: &'static str,
    pub st_rate_limited: &'static str,
    pub st_quota_exhausted: &'static str,
    pub st_transient_error: &'static str,
    pub st_auth_failed: &'static str,
    pub st_disabled: &'static str,
    pub st_unknown: &'static str,
    /// 状态是健康的, 但用户自己设的 token 限额满了 (不是 `SubscriptionState`)。
    pub st_quota_reached: &'static str,

    pub q_daily: &'static str,
    pub q_weekly: &'static str,
    pub q_monthly: &'static str,
    pub q_total: &'static str,

    pub toast_reconnected: &'static str,
    pub toast_load_failed: fn(reason: &str) -> String,
}

impl Strings {
    pub fn state(&self, state: SubscriptionState) -> &'static str {
        match state {
            SubscriptionState::Healthy => self.st_healthy,
            SubscriptionState::RateLimited => self.st_rate_limited,
            SubscriptionState::QuotaExhausted => self.st_quota_exhausted,
            SubscriptionState::TransientError => self.st_transient_error,
            SubscriptionState::AuthFailed => self.st_auth_failed,
            SubscriptionState::Disabled => self.st_disabled,
            SubscriptionState::Unknown => self.st_unknown,
        }
    }

    pub fn quota_period(&self, period: QuotaPeriod) -> &'static str {
        match period {
            QuotaPeriod::Daily => self.q_daily,
            QuotaPeriod::Weekly => self.q_weekly,
            QuotaPeriod::Monthly => self.q_monthly,
            QuotaPeriod::Total | QuotaPeriod::Unknown => self.q_total,
        }
    }
}

pub const ZH: Strings = Strings {
    tabs: ["总览", "订阅", "虚拟模型", "实时路由", "日志"],
    conn_connecting: "连接中",
    conn_connected: "已连接",
    conn_reconnecting: "重连中",

    key_switch_tab: "切页",
    key_refresh: "刷新",
    key_help: "帮助",
    key_quit: "退出",
    key_close: "关闭",

    help_title: "键位",
    help_rows: &[
        ("1-5", "直达对应页面"),
        ("Tab / Shift+Tab", "下一页 / 上一页"),
        ("r", "刷新当前页面"),
        ("?", "打开 / 关闭本帮助"),
        ("Esc", "关闭弹窗"),
        ("q / Ctrl+C", "退出"),
    ],

    too_small: "请放大终端窗口（至少 80×24）",
    coming_soon: "此页面将在后续版本提供",
    loading: "加载中",
    version_mismatch: |tui, app| format!("终端界面版本 {tui} 与 app 版本 {app} 不一致，请在桌面 app 的设置页重新添加到 PATH"),

    ov_today: "今日",
    ov_requests: "请求",
    ov_success_rate: "成功率",
    ov_tokens: "Token",
    ov_hourly: "每小时请求",
    ov_health: "订阅健康度",
    ov_auth_on: "鉴权 开启",
    ov_auth_off: "鉴权 关闭",
    ov_listen_all: "监听 0.0.0.0",
    ov_subs_summary: |total, ok| format!("{total} 个订阅 · {ok} 个可调度"),
    ov_no_subs: "还没有订阅，请先在桌面 app 里添加",
    ov_more_rows: |n| format!("… 还有 {n} 个"),

    st_healthy: "正常",
    st_rate_limited: "限流",
    st_quota_exhausted: "配额耗尽",
    st_transient_error: "临时错误",
    st_auth_failed: "凭证失效",
    st_disabled: "已禁用",
    st_unknown: "未知",
    st_quota_reached: "已达限额",

    q_daily: "日限额",
    q_weekly: "周限额",
    q_monthly: "月限额",
    q_total: "总限额",

    toast_reconnected: "已重新连接",
    toast_load_failed: |reason| format!("加载失败：{reason}"),
};

pub fn strings(lang: Lang) -> &'static Strings {
    match lang {
        // P6 在这里接上 EN / JA 两个 const; 在那之前三种语言都显示中文。
        Lang::Zh | Lang::En | Lang::Ja => &ZH,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn explicit_preference_wins_over_system() {
        assert_eq!(Lang::resolve("ja", env(&[("LANG", "zh_CN.UTF-8")])), Lang::Ja);
        assert_eq!(Lang::resolve("en", env(&[("LANG", "zh_CN.UTF-8")])), Lang::En);
    }

    #[test]
    fn system_follows_the_same_prefix_rule_as_the_desktop_app() {
        assert_eq!(Lang::resolve("system", env(&[("LANG", "zh_CN.UTF-8")])), Lang::Zh);
        assert_eq!(Lang::resolve("system", env(&[("LANG", "zh-Hant-TW")])), Lang::Zh);
        assert_eq!(Lang::resolve("system", env(&[("LANG", "ja_JP.UTF-8")])), Lang::Ja);
        assert_eq!(Lang::resolve("system", env(&[("LANG", "de_DE.UTF-8")])), Lang::En);
        assert_eq!(Lang::resolve("system", env(&[])), Lang::En);
    }

    #[test]
    fn lc_all_beats_lang_and_empty_values_are_skipped() {
        assert_eq!(Lang::resolve("system", env(&[("LC_ALL", "ja_JP"), ("LANG", "zh_CN")])), Lang::Ja);
        assert_eq!(Lang::resolve("system", env(&[("LC_ALL", ""), ("LANG", "zh_CN")])), Lang::Zh);
    }

    /// 标签栏一行放得下: 每个标签渲染成 ` N 名称 `, 之间一个分隔符, 总宽 ≤ 76 (80 列减边框与内距)。
    #[test]
    fn tab_bar_fits_in_80_columns() {
        for lang in [Lang::Zh, Lang::En, Lang::Ja] {
            let s = strings(lang);
            let total: usize = s.tabs.iter().map(|t| t.width() + 4).sum::<usize>() + (s.tabs.len() - 1);
            assert!(total <= 76, "{lang:?}: 标签栏宽 {total}");
        }
    }
}
