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
    pub key_select: &'static str,
    pub key_detail: &'static str,
    pub key_back: &'static str,
    pub key_toggle: &'static str,
    pub key_test: &'static str,
    pub key_models: &'static str,
    pub key_balance: &'static str,

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
    /// 断线时按 e/t/m/b 的提示。
    pub toast_offline: &'static str,
    pub toast_enabled: fn(name: &str) -> String,
    pub toast_disabled: fn(name: &str) -> String,
    /// `model` 为 `None` 时 (网络错误等测不出具体 model) 只显示前半句。
    pub toast_test_ok: fn(name: &str, model: Option<&str>) -> String,
    pub toast_test_failed: fn(name: &str, message: &str) -> String,
    pub toast_models_ok: fn(name: &str, n: usize) -> String,
    pub toast_models_manual: fn(name: &str, reason: &str) -> String,
    pub toast_balance_ok: fn(name: &str) -> String,
    pub toast_balance_failed: fn(name: &str, reason: &str) -> String,
    pub toast_mutation_failed: fn(name: &str, message: &str) -> String,

    pub sub_title: fn(usize) -> String,
    pub sub_col_name: &'static str,
    pub sub_col_provider: &'static str,
    pub sub_col_sonnet: &'static str,
    pub sub_f_state: &'static str,
    pub sub_f_provider: &'static str,
    pub sub_f_endpoint: &'static str,
    pub sub_f_slots: &'static str,
    pub sub_f_quota: &'static str,
    pub sub_f_balance: &'static str,
    pub sub_f_models: &'static str,
    pub sub_f_referenced: &'static str,
    pub sub_f_last_error: &'static str,
    pub sub_slot_fallback: &'static str,
    pub sub_slot_unset: &'static str,
    pub sub_effort_auto: &'static str,
    pub sub_balance_unsupported: &'static str,
    pub sub_balance_never: &'static str,
    pub sub_balance_unavailable: &'static str,
    pub sub_models_cached: fn(usize) -> String,
    pub sub_models_never: &'static str,
    pub sub_unreferenced: &'static str,
    pub sub_help_rows: &'static [(&'static str, &'static str)],
    /// 详情面板「状态」行后面追加的进行中文案 (busy 行)。
    pub sub_busy_toggling: &'static str,
    pub sub_busy_testing: &'static str,
    pub sub_busy_models: &'static str,
    pub sub_busy_balance: &'static str,
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
    key_select: "选择",
    key_detail: "详情",
    key_back: "返回",
    key_toggle: "启停",
    key_test: "测试",
    key_models: "模型",
    key_balance: "余额",

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
    toast_offline: "未连接,暂时无法操作",
    toast_enabled: |name| format!("已启用 {name}"),
    toast_disabled: |name| format!("已停用 {name}"),
    toast_test_ok: |name, model| match model {
        Some(model) => format!("{name}：连接正常 ({model})"),
        None => format!("{name}：连接正常"),
    },
    toast_test_failed: |name, message| format!("{name}：{message}"),
    toast_models_ok: |name, n| format!("{name}：获取到 {n} 个模型"),
    toast_models_manual: |name, reason| format!("{name}：无法自动获取模型 ({reason})"),
    toast_balance_ok: |name| format!("{name}：余额已刷新"),
    toast_balance_failed: |name, reason| format!("{name}：余额查询失败 ({reason})"),
    toast_mutation_failed: |name, message| format!("{name}：操作失败 ({message})"),

    sub_title: |n| format!("订阅 ({n})"),
    sub_col_name: "备注名",
    sub_col_provider: "厂商",
    sub_col_sonnet: "sonnet",
    sub_f_state: "状态",
    sub_f_provider: "厂商",
    sub_f_endpoint: "端点",
    sub_f_slots: "槽位",
    sub_f_quota: "限额",
    sub_f_balance: "余额",
    sub_f_models: "模型",
    sub_f_referenced: "被引用",
    sub_f_last_error: "最近错误",
    sub_slot_fallback: "兜底",
    sub_slot_unset: "(未配置)",
    sub_effort_auto: "auto",
    sub_balance_unsupported: "该厂商不支持余额查询",
    sub_balance_never: "还没查过,按 b 刷新",
    sub_balance_unavailable: "账户不可用 (可能欠费或被封)",
    sub_models_cached: |n| format!("已缓存 {n} 个"),
    sub_models_never: "还没获取过,按 m 刷新",
    sub_unreferenced: "没有被任何虚拟模型引用",
    sub_help_rows: &[
        ("↑↓ / j k", "上一条 / 下一条"),
        ("g / G", "第一条 / 最后一条"),
        ("PgUp / PgDn", "翻页"),
        ("⏎ / Esc", "进入 / 退出详情 (窄终端)"),
        ("e", "启用 / 停用"),
        ("t", "测试连接"),
        ("m", "刷新模型列表"),
        ("b", "刷新余额"),
    ],
    sub_busy_toggling: "正在切换…",
    sub_busy_testing: "正在测试连接…",
    sub_busy_models: "正在获取模型…",
    sub_busy_balance: "正在查询余额…",
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
