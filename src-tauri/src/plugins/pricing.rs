//! 计价注册表: 已启用的计价插件, 按优先级排好 (设计 §5)。费用在**展示时**用它现算。

use cc_router_plugin_spec::pricing_plugin::{PriceRule, PricingTable};
use serde::Serialize;

use crate::provider::model::AuthType;
use crate::subscription::model::Billing;

/// 价格来自哪个插件 (界面写明「价格来自 <插件> <版本>」)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PricingSource {
    pub plugin_id: String,
    pub version: String,
}

#[derive(Debug, Clone, Default)]
pub struct PricingRegistry {
    /// 按优先级从高到低; 同优先级按插件 id
    tables: Vec<(PricingSource, PricingTable)>,
}

pub struct PriceMatch<'a> {
    pub source: &'a PricingSource,
    pub table: &'a PricingTable,
    pub rule: &'a PriceRule,
}

/// 一条请求 / 一组聚合行的 token (输入是上游原值, 由 [`cost_of`] 按 auth_type 归一)。
#[derive(Debug, Clone, Copy, Default)]
pub struct Tokens {
    pub input_raw: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// 一笔费用 (micros = 该币种的百万分之一)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CostLine {
    pub currency: String,
    pub micros: i64,
    /// 包月订阅: 这是「等价按量费用」, 不是真实花费
    pub equivalent: bool,
    pub source: PricingSource,
}

impl PricingRegistry {
    /// `plugins`: (来源, 价格表, 优先级)。
    pub fn new(mut plugins: Vec<(PricingSource, PricingTable, i64)>) -> Self {
        plugins.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.plugin_id.cmp(&b.0.plugin_id)));
        Self { tables: plugins.into_iter().map(|(s, t, _)| (s, t)).collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    /// 第一个有匹配的插件胜出 (优先级高的覆盖低的)。
    pub fn lookup(&self, provider: &str, model: &str) -> Option<PriceMatch<'_>> {
        self.tables
            .iter()
            .find_map(|(source, table)| table.lookup(provider, model).map(|rule| PriceMatch { source, table, rule }))
    }

    pub fn sources(&self) -> Vec<PricingSource> {
        self.tables.iter().map(|(s, _)| s.clone()).collect()
    }
}

/// 按当前价格表算一笔费用。没有匹配的价格 → None。
/// 输入 token 与请求摘要同一口径 (`live_feed::uncached_input`): Anthropic 透传的输入本来就不含缓存,
/// 翻译类上游的输入含缓存, 要减掉。
pub fn cost_of(
    registry: &PricingRegistry,
    provider: &str,
    model: &str,
    auth_type: Option<AuthType>,
    billing: Billing,
    t: Tokens,
) -> Option<CostLine> {
    let m = registry.lookup(provider, model)?;
    let input = crate::observability::live_feed::uncached_input(auth_type, t.input_raw, t.cache_read, t.cache_write);
    Some(CostLine {
        currency: m.table.currency.clone(),
        micros: m.table.cost_micros(m.rule, input, t.output, t.cache_read, t.cache_write),
        equivalent: billing == Billing::Flat,
        source: m.source.clone(),
    })
}

/// 每个订阅的 (鉴权类型, 包月 / 按量), 算费用时用。订阅已删就不在表里 (按原值、按量算)。
pub async fn subscription_contexts(
    state: &crate::state::AppState,
) -> std::collections::HashMap<String, (AuthType, Billing)> {
    let subs = state.subscriptions.read().await;
    let mut out = std::collections::HashMap::with_capacity(subs.len());
    for (id, rt) in subs.iter() {
        let g = rt.read().await;
        out.insert(id.to_string(), (g.row.auth_type, g.row.billing));
    }
    out
}

/// 汇总: 两种币种分开、真实花费与等价按量费用分开 (不做汇率换算, 不相加)。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CostTotals {
    pub usd_micros: i64,
    pub cny_micros: i64,
    pub equiv_usd_micros: i64,
    pub equiv_cny_micros: i64,
}

impl CostTotals {
    pub fn add(&mut self, line: &CostLine) {
        let slot = match (line.currency.as_str(), line.equivalent) {
            ("CNY", false) => &mut self.cny_micros,
            ("CNY", true) => &mut self.equiv_cny_micros,
            (_, false) => &mut self.usd_micros,
            (_, true) => &mut self.equiv_usd_micros,
        };
        *slot += line.micros;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_router_plugin_spec::pricing_plugin::parse;

    fn table(currency: &str, prices: &str) -> PricingTable {
        parse(&format!(r#"{{"pricingApi":1,"currency":"{currency}","prices":[{prices}]}}"#)).unwrap().0
    }

    fn src(id: &str) -> PricingSource {
        PricingSource { plugin_id: id.into(), version: "1.0.0".into() }
    }

    #[test]
    fn higher_priority_plugin_wins_and_lower_one_fills_gaps() {
        let reg = PricingRegistry::new(vec![
            (src("community-a"), table("USD", r#"{"provider":"*","model":"m1","input":1,"output":1},{"provider":"*","model":"m2","input":1,"output":1}"#), 0),
            (src("mine"), table("USD", r#"{"provider":"*","model":"m1","input":9,"output":9}"#), 10),
        ]);
        assert_eq!(reg.lookup("x", "m1").unwrap().source.plugin_id, "mine");
        assert_eq!(reg.lookup("x", "m2").unwrap().source.plugin_id, "community-a");
        assert!(reg.lookup("x", "m3").is_none());
    }

    #[test]
    fn translated_upstreams_have_cache_subtracted_and_flat_is_equivalent() {
        let reg = PricingRegistry::new(vec![(src("p"), table("USD", r#"{"provider":"*","model":"m","input":1,"output":0,"cache_read":0}"#), 0)]);
        let t = Tokens { input_raw: 1_000_000, output: 0, cache_read: 400_000, cache_write: 0 };
        // Anthropic 透传: 输入不含缓存, 全额计 = $1
        let pass = cost_of(&reg, "x", "m", Some(AuthType::ApiKey), Billing::Metered, t).unwrap();
        assert_eq!(pass.micros, 1_000_000);
        assert!(!pass.equivalent);
        // 翻译类: 输入含缓存, 减掉 40 万 = $0.6
        let tr = cost_of(&reg, "x", "m", Some(AuthType::OpenaiResponsesApiKey), Billing::Flat, t).unwrap();
        assert_eq!(tr.micros, 600_000);
        assert!(tr.equivalent);

        let mut totals = CostTotals::default();
        totals.add(&pass);
        totals.add(&tr);
        assert_eq!(totals, CostTotals { usd_micros: 1_000_000, equiv_usd_micros: 600_000, ..Default::default() });
    }
}
