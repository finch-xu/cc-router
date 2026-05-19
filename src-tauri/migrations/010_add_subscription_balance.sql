-- 订阅余额/套餐余量缓存表
-- mirror model_list_cache (001_init.sql L49-55), 但 PK 只用 subscription_id
-- 因为余额是账户级 (api_key 维度), 不分 endpoint.
-- payload_json 存 BalanceSnapshot 整体序列化结果, 含 is_available + entries[] + fetched_at.
CREATE TABLE subscription_balance_cache (
  subscription_id TEXT PRIMARY KEY,
  fetched_at INTEGER NOT NULL,
  payload_json TEXT NOT NULL
);

-- subscriptions 表加 balance_discovery 列: provider yaml snapshot.
-- nullable (老订阅 + 不支持余额查询的 provider 都是 NULL),
-- 非空时存 JSON 序列化的 BalanceDiscovery {enabled, url, method, parser, cache_ttl_minutes}.
ALTER TABLE subscriptions ADD COLUMN balance_discovery TEXT;
