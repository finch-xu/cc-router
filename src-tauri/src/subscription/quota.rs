//! Per-subscription token quota: config (`TokenQuotas`), in-memory usage (`QuotaUsage`),
//! and local-calendar period arithmetic. Pure logic, no IO; persistence lives in
//! `subscription::store`, accounting hook in `observability::request_log::run_consumer`,
//! dispatch gate in `SubscriptionRuntime::is_dispatchable`.
//!
//! Period boundaries follow the machine's local calendar (`chrono::Local`): daily = local
//! midnight, weekly = local Monday 00:00, monthly = local 1st 00:00. `Total` never rolls;
//! its `period_start` is the last manual reset (UNIX epoch when never reset).

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaPeriod {
    Daily,
    Weekly,
    Monthly,
    Total,
}

pub const ALL_PERIODS: [QuotaPeriod; 4] = [
    QuotaPeriod::Daily,
    QuotaPeriod::Weekly,
    QuotaPeriod::Monthly,
    QuotaPeriod::Total,
];

impl QuotaPeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Total => "total",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "daily" => Some(Self::Daily),
            "weekly" => Some(Self::Weekly),
            "monthly" => Some(Self::Monthly),
            "total" => Some(Self::Total),
            _ => None,
        }
    }
    /// 中文标签, 用于 503 摘要 / 事件 summary (UI 自己走 i18n, 不用这个).
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Daily => "每日",
            Self::Weekly => "每周",
            Self::Monthly => "每月",
            Self::Total => "累计",
        }
    }
}

/// Persisted in `subscriptions.token_quotas` (JSON). Same serde recipe as `SlotEfforts`:
/// every field optional + `skip_serializing_if`, so `'{}'` = no limits and unknown keys are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenQuotas {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl TokenQuotas {
    pub fn limit(&self, p: QuotaPeriod) -> Option<u64> {
        match p {
            QuotaPeriod::Daily => self.daily,
            QuotaPeriod::Weekly => self.weekly,
            QuotaPeriod::Monthly => self.monthly,
            QuotaPeriod::Total => self.total,
        }
    }
    pub fn is_empty(&self) -> bool {
        ALL_PERIODS.iter().all(|p| self.limit(*p).is_none())
    }
    /// First period (in ALL_PERIODS order) whose effective usage >= its limit.
    pub fn first_exceeded(&self, usage: &QuotaUsage, now: DateTime<Utc>) -> Option<QuotaPeriod> {
        self.first_exceeded_in(usage, now, &Local)
    }
    pub fn first_exceeded_in<Tz: TimeZone>(
        &self,
        usage: &QuotaUsage,
        now: DateTime<Utc>,
        tz: &Tz,
    ) -> Option<QuotaPeriod> {
        ALL_PERIODS.into_iter().find(|p| {
            self.limit(*p)
                .is_some_and(|limit| usage.effective_in(*p, now, tz).total() >= limit)
        })
    }
    pub fn any_exceeded(&self, usage: &QuotaUsage, now: DateTime<Utc>) -> bool {
        self.first_exceeded(usage, now).is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaBucket {
    pub period_start: DateTime<Utc>,
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
}

impl Default for QuotaBucket {
    fn default() -> Self {
        Self {
            period_start: DateTime::<Utc>::UNIX_EPOCH,
            input: 0,
            output: 0,
            cache_creation: 0,
            cache_read: 0,
        }
    }
}

impl QuotaBucket {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_creation + self.cache_read
    }
    fn zeroed(period_start: DateTime<Utc>) -> Self {
        Self {
            period_start,
            ..Default::default()
        }
    }
}

/// In-memory usage, 4 buckets. Source of truth for the dispatch gate; DB is a restart snapshot.
#[derive(Debug, Clone, Default)]
pub struct QuotaUsage {
    buckets: HashMap<QuotaPeriod, QuotaBucket>,
}

impl QuotaUsage {
    pub fn bucket(&self, p: QuotaPeriod) -> QuotaBucket {
        self.buckets.get(&p).cloned().unwrap_or_default()
    }
    pub fn set_bucket(&mut self, p: QuotaPeriod, b: QuotaBucket) {
        self.buckets.insert(p, b);
    }
    /// Reset every calendar bucket whose period_start no longer matches `now`'s period.
    pub fn roll_if_needed(&mut self, now: DateTime<Utc>) {
        self.roll_if_needed_in(now, &Local)
    }
    pub fn roll_if_needed_in<Tz: TimeZone>(&mut self, now: DateTime<Utc>, tz: &Tz) {
        for p in [QuotaPeriod::Daily, QuotaPeriod::Weekly, QuotaPeriod::Monthly] {
            let start = period_start_in(p, now, tz);
            let cur = self.buckets.entry(p).or_default();
            if cur.period_start != start {
                *cur = QuotaBucket::zeroed(start);
            }
        }
        self.buckets.entry(QuotaPeriod::Total).or_default();
    }
    pub fn add(&mut self, now: DateTime<Utc>, input: u64, output: u64, cache_creation: u64, cache_read: u64) {
        self.add_in(now, &Local, input, output, cache_creation, cache_read)
    }
    pub fn add_in<Tz: TimeZone>(
        &mut self,
        now: DateTime<Utc>,
        tz: &Tz,
        input: u64,
        output: u64,
        cache_creation: u64,
        cache_read: u64,
    ) {
        self.roll_if_needed_in(now, tz);
        for p in ALL_PERIODS {
            let b = self.buckets.entry(p).or_default();
            b.input += input;
            b.output += output;
            b.cache_creation += cache_creation;
            b.cache_read += cache_read;
        }
    }
    /// Read-only view: an expired calendar bucket reads as zero (period rolled but not yet mutated).
    pub fn effective(&self, p: QuotaPeriod, now: DateTime<Utc>) -> QuotaBucket {
        self.effective_in(p, now, &Local)
    }
    pub fn effective_in<Tz: TimeZone>(&self, p: QuotaPeriod, now: DateTime<Utc>, tz: &Tz) -> QuotaBucket {
        let b = self.bucket(p);
        if p == QuotaPeriod::Total {
            return b;
        }
        let start = period_start_in(p, now, tz);
        if b.period_start == start {
            b
        } else {
            QuotaBucket::zeroed(start)
        }
    }
    pub fn reset_total(&mut self, now: DateTime<Utc>) {
        self.buckets.insert(QuotaPeriod::Total, QuotaBucket::zeroed(now));
    }
}

pub fn period_start(p: QuotaPeriod, now: DateTime<Utc>) -> DateTime<Utc> {
    period_start_in(p, now, &Local)
}

pub fn period_start_in<Tz: TimeZone>(p: QuotaPeriod, now: DateTime<Utc>, tz: &Tz) -> DateTime<Utc> {
    if p == QuotaPeriod::Total {
        return DateTime::<Utc>::UNIX_EPOCH;
    }
    let local = now.with_timezone(tz);
    let date = local.date_naive();
    let day = match p {
        QuotaPeriod::Daily => date,
        QuotaPeriod::Weekly => {
            let back = date.weekday().num_days_from_monday() as i64;
            date - Duration::days(back)
        }
        QuotaPeriod::Monthly => date.with_day(1).expect("day 1 always valid"),
        QuotaPeriod::Total => unreachable!(),
    };
    let midnight = day.and_time(NaiveTime::MIN);
    // DST gap 时 single() 为 None; 取 earliest, 再退回 UTC 解释兜底.
    tz.from_local_datetime(&midnight)
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&midnight))
}

pub fn period_end(p: QuotaPeriod, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    period_end_in(p, now, &Local)
}

pub fn period_end_in<Tz: TimeZone>(p: QuotaPeriod, now: DateTime<Utc>, tz: &Tz) -> Option<DateTime<Utc>> {
    let start = period_start_in(p, now, tz);
    let start_local = start.with_timezone(tz).date_naive();
    let next = match p {
        QuotaPeriod::Total => return None,
        QuotaPeriod::Daily => start_local + Duration::days(1),
        QuotaPeriod::Weekly => start_local + Duration::days(7),
        QuotaPeriod::Monthly => {
            let (y, m) = (start_local.year(), start_local.month());
            let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
            chrono::NaiveDate::from_ymd_opt(ny, nm, 1).expect("valid first of month")
        }
    };
    let midnight = next.and_time(NaiveTime::MIN);
    Some(
        tz.from_local_datetime(&midnight)
            .earliest()
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|| Utc.from_utc_datetime(&midnight)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone, Utc};

    fn cst() -> FixedOffset {
        FixedOffset::east_opt(8 * 3600).unwrap()
    }

    #[test]
    fn daily_start_is_local_midnight() {
        // 2026-08-15 01:30 北京 = 2026-08-14 17:30 UTC; 本地日起点应为 08-15 00:00 CST = 08-14 16:00 UTC
        let now = Utc.with_ymd_and_hms(2026, 8, 14, 17, 30, 0).unwrap();
        let start = period_start_in(QuotaPeriod::Daily, now, &cst());
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 8, 14, 16, 0, 0).unwrap());
        // 同一时刻按 UTC 算则是 08-14 00:00 UTC —— 证明是本地边界不是 UTC 边界
        let start_utc = period_start_in(QuotaPeriod::Daily, now, &Utc);
        assert_eq!(start_utc, Utc.with_ymd_and_hms(2026, 8, 14, 0, 0, 0).unwrap());
    }

    #[test]
    fn weekly_start_is_local_monday() {
        // 2026-08-16 是周日. 北京 08-16 12:00 → 周起点 08-10 (周一) 00:00 CST = 08-09 16:00 UTC
        let now = Utc.with_ymd_and_hms(2026, 8, 16, 4, 0, 0).unwrap();
        let start = period_start_in(QuotaPeriod::Weekly, now, &cst());
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 8, 9, 16, 0, 0).unwrap());
        // 周一当天 00:30 仍归本周
        let mon = Utc.with_ymd_and_hms(2026, 8, 9, 16, 30, 0).unwrap();
        assert_eq!(period_start_in(QuotaPeriod::Weekly, mon, &cst()), start);
    }

    #[test]
    fn monthly_start_handles_year_boundary() {
        // 北京 2027-01-01 00:10 = 2026-12-31 16:10 UTC → 月起点 2027-01-01 00:00 CST = 2026-12-31 16:00 UTC
        let now = Utc.with_ymd_and_hms(2026, 12, 31, 16, 10, 0).unwrap();
        let start = period_start_in(QuotaPeriod::Monthly, now, &cst());
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 12, 31, 16, 0, 0).unwrap());
    }

    #[test]
    fn total_period_start_is_epoch_and_no_end() {
        let now = Utc::now();
        assert_eq!(
            period_start_in(QuotaPeriod::Total, now, &cst()),
            DateTime::<Utc>::UNIX_EPOCH
        );
        assert!(period_end(QuotaPeriod::Total, now).is_none());
    }

    #[test]
    fn roll_if_needed_resets_only_expired_buckets() {
        let t0 = Utc.with_ymd_and_hms(2026, 8, 14, 17, 30, 0).unwrap(); // 北京 08-15 01:30
        let mut u = QuotaUsage::default();
        u.add_in(t0, &cst(), 10, 20, 30, 40);
        assert_eq!(u.bucket(QuotaPeriod::Daily).total(), 100);
        assert_eq!(u.bucket(QuotaPeriod::Total).total(), 100);
        // 次日 (北京 08-16 01:30): daily 清零, weekly (同周) / monthly / total 保留
        let t1 = t0 + chrono::Duration::days(1);
        u.roll_if_needed_in(t1, &cst());
        assert_eq!(u.bucket(QuotaPeriod::Daily).total(), 0);
        assert_eq!(u.bucket(QuotaPeriod::Weekly).total(), 100);
        assert_eq!(u.bucket(QuotaPeriod::Monthly).total(), 100);
        assert_eq!(u.bucket(QuotaPeriod::Total).total(), 100);
    }

    #[test]
    fn effective_treats_expired_bucket_as_zero_without_mutation() {
        let t0 = Utc.with_ymd_and_hms(2026, 8, 14, 17, 30, 0).unwrap();
        let mut u = QuotaUsage::default();
        u.add_in(t0, &cst(), 50, 0, 0, 0);
        let t1 = t0 + chrono::Duration::days(1);
        assert_eq!(u.effective_in(QuotaPeriod::Daily, t1, &cst()).total(), 0);
        // 只读: 原桶不变
        assert_eq!(u.bucket(QuotaPeriod::Daily).total(), 50);
    }

    #[test]
    fn first_exceeded_uses_ge_and_ignores_unset() {
        let now = Utc.with_ymd_and_hms(2026, 8, 14, 17, 30, 0).unwrap();
        let mut u = QuotaUsage::default();
        u.add_in(now, &cst(), 100, 0, 0, 0);
        let none = TokenQuotas::default();
        assert!(none.first_exceeded_in(&u, now, &cst()).is_none());
        let q = TokenQuotas {
            daily: Some(100),
            ..Default::default()
        };
        assert_eq!(q.first_exceeded_in(&u, now, &cst()), Some(QuotaPeriod::Daily));
        let q2 = TokenQuotas {
            daily: Some(101),
            monthly: Some(1000),
            ..Default::default()
        };
        assert!(q2.first_exceeded_in(&u, now, &cst()).is_none());
    }

    #[test]
    fn reset_total_zeroes_and_moves_start() {
        let now = Utc::now();
        let mut u = QuotaUsage::default();
        u.add_in(now, &cst(), 1, 1, 1, 1);
        u.reset_total(now);
        assert_eq!(u.bucket(QuotaPeriod::Total).total(), 0);
        assert_eq!(u.bucket(QuotaPeriod::Total).period_start, now);
        // 其他桶不受影响
        assert_eq!(u.bucket(QuotaPeriod::Daily).total(), 4);
    }

    #[test]
    fn token_quotas_serde_matches_slot_efforts_recipe() {
        let q: TokenQuotas = serde_json::from_str("{}").unwrap();
        assert!(q.is_empty());
        let q: TokenQuotas = serde_json::from_str(r#"{"daily":5000000,"unknown":1}"#).unwrap();
        assert_eq!(q.daily, Some(5_000_000));
        assert_eq!(serde_json::to_string(&q).unwrap(), r#"{"daily":5000000}"#);
    }
}
