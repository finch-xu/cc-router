//! Receipts (用量小票) 数据聚合 command。
//!
//! 数据源是双路的 (v3.4 起):
//! - 7天/30天/1年/全部 → `receipt_stats_daily` 小票专用聚合表 (含 real_model_name 维度,
//!   永久保留, 不受 log_retention_days 清理; migration 015 建表并从 requests 回填)
//! - 「过去 24 小时」→ `requests` 原始表 (滚动窗口, daily 粒度做不到)。
//!   安全性: Settings 里保留期最短可选 7 天 (0=永久), requests 必然覆盖最近 24h,
//!   该分支不受日志清理影响。仅手改 settings.json 填 1-6 天才有理论边界。
//!
//! 与 statistics.rs 的差异: statistics 查 `request_stats_daily` (无 real_model 维度),
//! 服务概览/趋势图/heatmap; 这里要按真实模型下钻, 用自己的聚合表。
//!
//! Receipts 设计语义只展示 fable/opus/sonnet/haiku 四档主消费项, fallback 透传通道
//! 不计入小票, SQL 已 WHERE virtual_model_name IN (...) 过滤。

use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use sqlx::Row;
use tauri::State;

use crate::error::AppResult;
use crate::observability::request_log::{local_days_ago_key, local_days_ago_start_ms, now_ms, DAY_MS};
use crate::state::AppState;
use crate::virtual_model::model::VirtualModelName;

/// Receipts 专用的时间范围 enum。
///
/// 与 `StatsRange` 故意不共用——StatsRange 全走 daily 聚合表;
/// 这里 Last24Hours 查 requests 原始表 (滚动窗口), 其余走 receipt_stats_daily (本地日历日 key)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptRange {
    /// serde snake_case 不会在「数字→大写字母」边界插下划线: 默认会得到 "last24_hours",
    /// 与前端契约 "last_24_hours" 不符导致反序列化失败 (24h 小票永远空), 这里显式改名。
    #[serde(rename = "last_24_hours")]
    Last24Hours,
    Last7Days,
    Last30Days,
    LastYear,
    AllTime,
}

impl ReceiptRange {
    /// 「最近 N 天」包含今天共 N 天, 天数往回推; None = 不按天 (24h 滚动 / 全部)。
    fn days_back(self) -> Option<u32> {
        match self {
            Self::Last24Hours | Self::AllTime => None,
            Self::Last7Days => Some(6),
            Self::Last30Days => Some(29),
            Self::LastYear => Some(364),
        }
    }

    /// 范围起点瞬时 (ms): 24h 是滚动窗口 `now - 24h`; 按天的范围是起始本地日的 0 点瞬时
    /// (仅用于 requests 原始表过滤、小票展示与单号, 聚合表过滤走 `since_day`)。
    fn since_ms(self) -> i64 {
        let now = now_ms();
        match self {
            Self::Last24Hours => now - DAY_MS,
            Self::AllTime => 0,
            _ => local_days_ago_start_ms(now, self.days_back().unwrap_or(0)),
        }
    }

    /// 聚合表 (`receipt_stats_daily.day`, 本地日历日 key) 过滤下限, 空串 = 全部。
    fn since_day(self) -> String {
        match self.days_back() {
            Some(n) => local_days_ago_key(now_ms(), n),
            None => String::new(),
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct ReceiptTotalsDto {
    pub request_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
}

impl ReceiptTotalsDto {
    fn add(&mut self, other: &ReceiptTotalsDto) {
        self.request_count += other.request_count;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_tokens += other.cache_creation_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
    }
}

#[derive(Debug, Serialize)]
pub struct ReceiptSubItemDto {
    pub subscription_id: String,
    /// None 表示订阅已被删除; 前端按 i18n 显示「已删除订阅」兜底文案
    pub subscription_display_name: Option<String>,
    pub provider_id: String,
    pub provider_display_name: String,
    pub real_model_name: String,
    pub totals: ReceiptTotalsDto,
}

#[derive(Debug, Serialize)]
pub struct ReceiptVirtualModelItemDto {
    /// "model-fable" / "model-opus" / "model-sonnet" / "model-haiku" — fallback 不出现在小票
    pub virtual_model_name: String,
    pub subtotal: ReceiptTotalsDto,
    pub sub_items: Vec<ReceiptSubItemDto>,
}

#[derive(Debug, Serialize)]
pub struct ReceiptDto {
    pub range: ReceiptRange,
    /// inclusive 下限, ms
    pub range_start_ms: i64,
    /// exclusive 上限, ms (查询生成时间)
    pub range_end_ms: i64,
    pub generated_at_ms: i64,
    /// 8 位大写 hex, 用于小票上的「单号」展示
    pub slip_no: String,
    /// 始终 4 项: model-fable / model-opus / model-sonnet / model-haiku, 顺序固定, 空也返回
    pub items: Vec<ReceiptVirtualModelItemDto>,
    pub grand_total: ReceiptTotalsDto,
}

/// 固定排序: fable → opus → sonnet → haiku (与前端 virtualModels.ts::VM_ORDER 一致),
/// fallback 不出现
const RECEIPT_VM_ORDER: [VirtualModelName; 4] = [
    VirtualModelName::Fable,
    VirtualModelName::Opus,
    VirtualModelName::Sonnet,
    VirtualModelName::Haiku,
];

#[tauri::command]
pub async fn get_receipt_summary(
    state: State<'_, AppState>,
    range: ReceiptRange,
) -> AppResult<ReceiptDto> {
    let since = range.since_ms();
    let now = now_ms();

    let in_clause = RECEIPT_VM_ORDER
        .iter()
        .map(|v| format!("'{}'", v.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    // 两条路径列别名完全一致, 行 → DTO 映射共用。
    let sql = match range {
        // 滚动 24h 窗口只有原始表能做; 保留期最短 7 天, 该分支不受清理影响
        ReceiptRange::Last24Hours => format!(
            "SELECT
                r.virtual_model_name,
                r.subscription_id,
                sub.display_name                                    AS subscription_display_name,
                r.provider_id,
                COALESCE(sub.provider_display_name, r.provider_id) AS provider_display_name,
                r.real_model_name,
                COUNT(*)                                            AS request_count,
                COALESCE(SUM(r.upstream_input_tokens), 0)           AS input_tokens,
                COALESCE(SUM(r.upstream_output_tokens), 0)          AS output_tokens,
                COALESCE(SUM(r.upstream_cache_creation), 0)         AS cache_creation_tokens,
                COALESCE(SUM(r.upstream_cache_read), 0)             AS cache_read_tokens
             FROM requests r
             LEFT JOIN subscriptions sub ON sub.id = r.subscription_id
             WHERE r.timestamp >= ?
               AND r.virtual_model_name IN ({in_clause})
             GROUP BY r.virtual_model_name, r.subscription_id, r.real_model_name"
        ),
        // 7d/30d/1y 按本地日历日过滤聚合表 (`day >= since_day`), all_time 用空串
        _ => format!(
            "SELECT
                r.virtual_model_name,
                r.subscription_id,
                sub.display_name                                        AS subscription_display_name,
                MIN(r.provider_id)                                      AS provider_id,
                COALESCE(sub.provider_display_name, MIN(r.provider_id)) AS provider_display_name,
                r.real_model_name,
                COALESCE(SUM(r.request_count), 0)                       AS request_count,
                COALESCE(SUM(r.input_tokens), 0)                        AS input_tokens,
                COALESCE(SUM(r.output_tokens), 0)                       AS output_tokens,
                COALESCE(SUM(r.cache_creation_tokens), 0)               AS cache_creation_tokens,
                COALESCE(SUM(r.cache_read_tokens), 0)                   AS cache_read_tokens
             FROM receipt_stats_daily r
             LEFT JOIN subscriptions sub ON sub.id = r.subscription_id
             WHERE r.day >= ?
               AND r.virtual_model_name IN ({in_clause})
             GROUP BY r.virtual_model_name, r.subscription_id, r.real_model_name"
        ),
    };
    let rows = match range {
        ReceiptRange::Last24Hours => sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).bind(since).fetch_all(&state.db).await?,
        _ => sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).bind(range.since_day()).fetch_all(&state.db).await?,
    };

    let mut buckets: std::collections::HashMap<String, Vec<ReceiptSubItemDto>> =
        std::collections::HashMap::new();

    for r in rows {
        let totals = ReceiptTotalsDto {
            request_count: r.try_get("request_count")?,
            input_tokens: r.try_get("input_tokens")?,
            output_tokens: r.try_get("output_tokens")?,
            cache_creation_tokens: r.try_get("cache_creation_tokens")?,
            cache_read_tokens: r.try_get("cache_read_tokens")?,
        };
        let sub_item = ReceiptSubItemDto {
            subscription_id: r.try_get("subscription_id")?,
            subscription_display_name: r.try_get("subscription_display_name")?,
            provider_id: r.try_get("provider_id")?,
            provider_display_name: r.try_get("provider_display_name")?,
            real_model_name: r.try_get("real_model_name")?,
            totals,
        };
        let vm: String = r.try_get("virtual_model_name")?;
        buckets.entry(vm).or_default().push(sub_item);
    }

    let mut items = Vec::with_capacity(RECEIPT_VM_ORDER.len());
    let mut grand_total = ReceiptTotalsDto::default();

    for vm in RECEIPT_VM_ORDER {
        let mut sub_items = buckets.remove(vm.as_str()).unwrap_or_default();
        // sub_items 内按 request_count 降序, 让用得多的订阅排前面 (小票阅读视角)
        sub_items.sort_by(|a, b| b.totals.request_count.cmp(&a.totals.request_count));

        let mut subtotal = ReceiptTotalsDto::default();
        for s in &sub_items {
            subtotal.add(&s.totals);
        }
        grand_total.add(&subtotal);

        items.push(ReceiptVirtualModelItemDto {
            virtual_model_name: vm.as_str().to_string(),
            subtotal,
            sub_items,
        });
    }

    let slip_no = compute_slip_no(since, now);

    Ok(ReceiptDto {
        range,
        range_start_ms: since,
        range_end_ms: now,
        generated_at_ms: now,
        slip_no,
        items,
        grand_total,
    })
}

/// 8 位大写 hex, 来自 stdlib DefaultHasher(start_ms, end_ms) 取低 32 位。
/// 不掺 generated_at, 同范围连续刷新出同一个号 — 让用户可以重复出同一张小票。
/// 这里 hash 值仅用于「单号展示」,不需要密码学强度,免去新增 sha1 依赖。
fn compute_slip_no(start_ms: i64, end_ms: i64) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    start_ms.hash(&mut hasher);
    end_ms.hash(&mut hasher);
    let h = hasher.finish();
    format!("{:08X}", (h & 0xFFFF_FFFF) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slip_no_is_8_uppercase_hex() {
        let s = compute_slip_no(1_700_000_000_000, 1_700_086_400_000);
        assert_eq!(s.len(), 8);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
    }

    #[test]
    fn slip_no_stable_for_same_range() {
        let a = compute_slip_no(1_000, 2_000);
        let b = compute_slip_no(1_000, 2_000);
        assert_eq!(a, b);
    }

    #[test]
    fn receipt_totals_add_accumulates() {
        let mut a = ReceiptTotalsDto {
            request_count: 1,
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_tokens: 2,
            cache_read_tokens: 1,
        };
        let b = ReceiptTotalsDto {
            request_count: 3,
            input_tokens: 30,
            output_tokens: 15,
            cache_creation_tokens: 6,
            cache_read_tokens: 3,
        };
        a.add(&b);
        assert_eq!(a.request_count, 4);
        assert_eq!(a.input_tokens, 40);
        assert_eq!(a.output_tokens, 20);
        assert_eq!(a.cache_creation_tokens, 8);
        assert_eq!(a.cache_read_tokens, 4);
    }

    #[test]
    fn since_ms_last_24_hours_is_rolling() {
        let r = ReceiptRange::Last24Hours.since_ms();
        let now = now_ms();
        // 应该在 (now - DAY_MS, now) 之间, 容忍 100ms 抖动
        assert!(r <= now);
        assert!(r >= now - DAY_MS - 100);
        // 不对齐 UTC 0 点 (除非恰好在 UTC 0 点附近运行, 概率极低)
        // 这里只验证窗口尺寸合理, 不做严格对齐断言
    }

    #[test]
    fn since_ms_all_time_is_zero() {
        assert_eq!(ReceiptRange::AllTime.since_ms(), 0);
        assert_eq!(ReceiptRange::AllTime.since_day(), "");
    }

    /// 前后端契约回归测试: 前端 src/types.ts::ReceiptRange 用的是
    /// "last_24_hours" / "last7_days" / "last30_days" / "last_year" / "all_time"。
    /// serde snake_case 对含数字的 variant 不会在数字后插下划线
    /// ("Last24Hours" 默认 → "last24_hours"), 必须靠显式 rename 对齐。
    #[test]
    fn receipt_range_serde_names_match_frontend_contract() {
        let cases = [
            (ReceiptRange::Last24Hours, "last_24_hours"),
            (ReceiptRange::Last7Days, "last7_days"),
            (ReceiptRange::Last30Days, "last30_days"),
            (ReceiptRange::LastYear, "last_year"),
            (ReceiptRange::AllTime, "all_time"),
        ];
        for (range, name) in cases {
            // 序列化 (后端 → 前端 ReceiptDto.range)
            assert_eq!(
                serde_json::to_string(&range).unwrap(),
                format!("\"{name}\"")
            );
            // 反序列化 (前端 → 后端 command 参数)
            let parsed: ReceiptRange =
                serde_json::from_str(&format!("\"{name}\"")).unwrap();
            assert_eq!(parsed, range);
        }
    }

    #[test]
    fn since_day_is_local_calendar_key_matching_since_ms() {
        use crate::observability::request_log::local_day_key;
        // 按天的范围: since_ms 就是 since_day 那一天的本地 0 点, 两者自洽
        for r in [ReceiptRange::Last7Days, ReceiptRange::Last30Days, ReceiptRange::LastYear] {
            let day = r.since_day();
            assert_eq!(day.len(), 10, "YYYY-MM-DD");
            assert_eq!(local_day_key(r.since_ms()), day);
            assert_ne!(local_day_key(r.since_ms() - 1), day);
        }
    }
}
