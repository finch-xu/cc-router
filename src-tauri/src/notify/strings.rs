//! 通知默认文案表 (四语, 通知插件的 `event.title` / `event.text` 与内置桌面通知共用, 设计 §7.3 / §7.5)。
//!
//! 沿用 `tray/strings.rs` 的写法: 四个 `const`, 漏字段编译期报错; `ZH_HANT` 由 `zh_hant_check` 锁住零简体残留。

use crate::tray::TrayLocale;

pub struct NotifyStrings {
    pub auth_failed_title: fn(sub: &str) -> String,
    pub auth_failed_text: fn(provider: &str, reason: &str) -> String,
    pub cooldown_title: fn(sub: &str) -> String,
    pub cooldown_text: fn(provider: &str, reason: &str) -> String,
    pub recovered_title: fn(sub: &str) -> String,
    pub recovered_text: fn(provider: &str) -> String,
    pub quota_reached_title: fn(sub: &str) -> String,
    pub quota_reached_text: fn(period: &str, used: u64, limit: u64) -> String,
    pub quota_warning_title: fn(sub: &str, percent: u64) -> String,
    pub quota_warning_text: fn(period: &str, used: u64, limit: u64, percent: u64) -> String,
    pub balance_low_title: fn(sub: &str) -> String,
    pub balance_low_text: fn(provider: &str, remaining: &str, unit: &str) -> String,
    pub all_failed_title: fn(vm: &str) -> String,
    pub all_failed_text: fn(vm: &str, count: u64) -> String,
    pub proxy_stopped_title: &'static str,
    pub proxy_stopped_text: fn(reason: &str) -> String,
    pub update_available_title: fn(version: &str) -> String,
    pub update_available_text: fn(version: &str) -> String,
    pub daily_summary_title: &'static str,
    pub daily_summary_text: fn(requests: u64, tokens: u64, cost: &str) -> String,
    pub test_title: &'static str,
    pub test_text: &'static str,
}

pub fn strings_for(locale: TrayLocale) -> &'static NotifyStrings {
    match locale {
        TrayLocale::Zh => &ZH,
        TrayLocale::ZhHant => &ZH_HANT,
        TrayLocale::En => &EN,
        TrayLocale::Ja => &JA,
    }
}

pub const ZH: NotifyStrings = NotifyStrings {
    auth_failed_title: |sub| format!("订阅失效：{sub}"),
    auth_failed_text: |provider, reason| format!("{provider} 鉴权失败：{reason}"),
    cooldown_title: |sub| format!("订阅进入冷却：{sub}"),
    cooldown_text: |provider, reason| format!("{provider} 暂时不可用：{reason}"),
    recovered_title: |sub| format!("订阅已恢复：{sub}"),
    recovered_text: |provider| format!("{provider} 已恢复正常调度"),
    quota_reached_title: |sub| format!("已达 Token 限额：{sub}"),
    quota_reached_text: |period, used, limit| format!("{period} 用量已达上限（{used} / {limit}），已暂停调度至下一周期"),
    quota_warning_title: |sub, pct| format!("Token 限额预警（{pct}%）：{sub}"),
    quota_warning_text: |period, used, limit, pct| format!("{period} 用量已达 {pct}%（{used} / {limit}）"),
    balance_low_title: |sub| format!("订阅余额不足：{sub}"),
    balance_low_text: |provider, rem, unit| format!("{provider} 当前剩余 {rem} {unit}"),
    all_failed_title: |vm| format!("全部候选失败：{vm}"),
    all_failed_text: |vm, count| format!("{vm} 最近 {count} 次请求均无可用订阅（HTTP 503）"),
    proxy_stopped_title: "代理服务已停止",
    proxy_stopped_text: |reason| format!("本地代理监听意外退出：{reason}"),
    update_available_title: |v| format!("发现新版本 v{v}"),
    update_available_text: |v| format!("cc-router v{v} 已发布，可在关于页查看并更新"),
    daily_summary_title: "cc-router 今日用量汇总",
    daily_summary_text: |reqs, tokens, cost| {
        if cost.is_empty() {
            format!("今日共 {reqs} 次请求，合计 {tokens} tokens")
        } else {
            format!("今日共 {reqs} 次请求，合计 {tokens} tokens，估算费用 {cost}")
        }
    },
    test_title: "cc-router 测试通知",
    test_text: "如果你看到了这条消息，说明通知通道工作正常。",
};

pub const ZH_HANT: NotifyStrings = NotifyStrings {
    auth_failed_title: |sub| format!("訂閱失效：{sub}"),
    auth_failed_text: |provider, reason| format!("{provider} 驗證失敗：{reason}"),
    cooldown_title: |sub| format!("訂閱進入冷卻：{sub}"),
    cooldown_text: |provider, reason| format!("{provider} 暫時無法使用：{reason}"),
    recovered_title: |sub| format!("訂閱已恢復：{sub}"),
    recovered_text: |provider| format!("{provider} 已恢復正常排程"),
    quota_reached_title: |sub| format!("已達 Token 限額：{sub}"),
    quota_reached_text: |period, used, limit| format!("{period} 用量已達上限（{used} / {limit}），已暫停排程至下一週期"),
    quota_warning_title: |sub, pct| format!("Token 限額預警（{pct}%）：{sub}"),
    quota_warning_text: |period, used, limit, pct| format!("{period} 用量已達 {pct}%（{used} / {limit}）"),
    balance_low_title: |sub| format!("訂閱餘額不足：{sub}"),
    balance_low_text: |provider, rem, unit| format!("{provider} 目前剩餘 {rem} {unit}"),
    all_failed_title: |vm| format!("全部候選失敗：{vm}"),
    all_failed_text: |vm, count| format!("{vm} 最近 {count} 次請求皆無可用訂閱（HTTP 503）"),
    proxy_stopped_title: "代理服務已停止",
    proxy_stopped_text: |reason| format!("本機代理監聽意外結束：{reason}"),
    update_available_title: |v| format!("發現新版本 v{v}"),
    update_available_text: |v| format!("cc-router v{v} 已發布，可在關於頁檢視並更新"),
    daily_summary_title: "cc-router 今日用量摘要",
    daily_summary_text: |reqs, tokens, cost| {
        if cost.is_empty() {
            format!("今日共 {reqs} 次請求，合計 {tokens} tokens")
        } else {
            format!("今日共 {reqs} 次請求，合計 {tokens} tokens，估算費用 {cost}")
        }
    },
    test_title: "cc-router 測試通知",
    test_text: "如果你看到了這則訊息，代表通知管道運作正常。",
};

pub const EN: NotifyStrings = NotifyStrings {
    auth_failed_title: |sub| format!("Subscription auth failed: {sub}"),
    auth_failed_text: |provider, reason| format!("{provider} rejected credentials: {reason}"),
    cooldown_title: |sub| format!("Subscription in cooldown: {sub}"),
    cooldown_text: |provider, reason| format!("{provider} temporarily unavailable: {reason}"),
    recovered_title: |sub| format!("Subscription recovered: {sub}"),
    recovered_text: |provider| format!("{provider} is healthy and back in rotation"),
    quota_reached_title: |sub| format!("Token quota reached: {sub}"),
    quota_reached_text: |period, used, limit| format!("{period} usage reached its limit ({used} / {limit}); paused until next period"),
    quota_warning_title: |sub, pct| format!("Token quota warning ({pct}%): {sub}"),
    quota_warning_text: |period, used, limit, pct| format!("{period} usage reached {pct}% ({used} / {limit})"),
    balance_low_title: |sub| format!("Low balance: {sub}"),
    balance_low_text: |provider, rem, unit| format!("{provider} remaining balance: {rem} {unit}"),
    all_failed_title: |vm| format!("All candidates failed: {vm}"),
    all_failed_text: |vm, count| format!("{vm} had {count} request(s) with no available subscription (HTTP 503)"),
    proxy_stopped_title: "Proxy service stopped",
    proxy_stopped_text: |reason| format!("Local proxy listener stopped unexpectedly: {reason}"),
    update_available_title: |v| format!("Update available: v{v}"),
    update_available_text: |v| format!("cc-router v{v} is ready to install from the About page"),
    daily_summary_title: "cc-router daily usage summary",
    daily_summary_text: |reqs, tokens, cost| {
        if cost.is_empty() {
            format!("{reqs} requests today, {tokens} tokens total")
        } else {
            format!("{reqs} requests today, {tokens} tokens total, estimated cost {cost}")
        }
    },
    test_title: "cc-router test notification",
    test_text: "If you can read this, notifications are working.",
};

pub const JA: NotifyStrings = NotifyStrings {
    auth_failed_title: |sub| format!("サブスク認証失敗：{sub}"),
    auth_failed_text: |provider, reason| format!("{provider} の認証に失敗しました：{reason}"),
    cooldown_title: |sub| format!("サブスクがクールダウン中：{sub}"),
    cooldown_text: |provider, reason| format!("{provider} が一時的に利用できません：{reason}"),
    recovered_title: |sub| format!("サブスクが復旧しました：{sub}"),
    recovered_text: |provider| format!("{provider} が正常なルーティングに復帰しました"),
    quota_reached_title: |sub| format!("トークン上限に到達：{sub}"),
    quota_reached_text: |period, used, limit| format!("{period} の使用量が上限（{used} / {limit}）に達したため、次の期間まで一時停止します"),
    quota_warning_title: |sub, pct| format!("トークン上限警告（{pct}%）：{sub}"),
    quota_warning_text: |period, used, limit, pct| format!("{period} の使用量が {pct}%（{used} / {limit}）に達しました"),
    balance_low_title: |sub| format!("残高が少なくなっています：{sub}"),
    balance_low_text: |provider, rem, unit| format!("{provider} の残高：{rem} {unit}"),
    all_failed_title: |vm| format!("すべての候補が失敗：{vm}"),
    all_failed_text: |vm, count| format!("{vm} で利用可能なサブスクがないリクエストが {count} 件発生しました（HTTP 503）"),
    proxy_stopped_title: "プロキシサービスが停止しました",
    proxy_stopped_text: |reason| format!("ローカルプロキシが予期せず終了しました：{reason}"),
    update_available_title: |v| format!("新しいバージョン v{v}"),
    update_available_text: |v| format!("cc-router v{v} が公開されました。バージョン情報から更新できます"),
    daily_summary_title: "cc-router 本日の利用サマリー",
    daily_summary_text: |reqs, tokens, cost| {
        if cost.is_empty() {
            format!("本日のリクエスト {reqs} 件、合計 {tokens} トークン")
        } else {
            format!("本日のリクエスト {reqs} 件、合計 {tokens} トークン、推定費用 {cost}")
        }
    },
    test_title: "cc-router テスト通知",
    test_text: "このメッセージが表示されていれば通知は正常に動作しています。",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zh_hant_has_no_simplified_residue() {
        let block = crate::zh_hant_check::const_block(include_str!("strings.rs"), "ZH_HANT");
        let residue = crate::zh_hant_check::simplified_residue(&block);
        assert!(residue.is_empty(), "ZH_HANT 里有简体字: {residue:?}");
    }

    #[test]
    fn every_locale_renders_non_empty_messages() {
        for loc in [TrayLocale::Zh, TrayLocale::ZhHant, TrayLocale::En, TrayLocale::Ja] {
            let s = strings_for(loc);
            assert!(!(s.auth_failed_title)("sub").is_empty());
            assert!(!(s.auth_failed_text)("prov", "401").is_empty());
            assert!(!(s.cooldown_title)("sub").is_empty());
            assert!(!(s.recovered_title)("sub").is_empty());
            assert!(!(s.quota_reached_text)("daily", 10, 10).is_empty());
            assert!(!(s.quota_warning_title)("sub", 80).is_empty());
            assert!(!(s.balance_low_text)("prov", "1.5", "USD").is_empty());
            assert!(!(s.all_failed_text)("model-opus", 3).is_empty());
            assert!(!s.proxy_stopped_title.is_empty());
            assert!(!(s.update_available_title)("7.1.0").is_empty());
            assert!(!(s.daily_summary_text)(10, 5000, "$0.12").is_empty());
            assert!(!s.test_title.is_empty());
            assert!(!s.test_text.is_empty());
        }
    }
}
