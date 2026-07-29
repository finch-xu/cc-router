-- 小票专用日聚合表, 与 request_stats_daily 平级共生 (flush 同事务 UPSERT)
-- 差异: 多 real_model_name 维度 (小票要按真实模型下钻), 只存小票需要的 5 个计数列
-- 永久保留, 不受 log_retention_days 清理 -- 日志表被清理/缩短保留期后小票数据不受影响
-- provider_id 不在 PK 里, 语义同 request_stats_daily: 取该聚合桶首次写入的值

CREATE TABLE receipt_stats_daily (
  date_utc           INTEGER NOT NULL,
  virtual_model_name TEXT    NOT NULL,
  subscription_id    TEXT    NOT NULL,
  real_model_name    TEXT    NOT NULL,
  provider_id        TEXT    NOT NULL,

  request_count         INTEGER NOT NULL DEFAULT 0,
  input_tokens          INTEGER NOT NULL DEFAULT 0,
  output_tokens         INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens     INTEGER NOT NULL DEFAULT 0,

  PRIMARY KEY (date_utc, virtual_model_name, subscription_id, real_model_name)
);

CREATE INDEX idx_receipt_stats_daily_date ON receipt_stats_daily(date_utc DESC);

-- 从 requests 现存数据回填, 拿到真实 real_model_name -- 迁移前后小票输出一致
-- 口径与旧版小票 SQL 相同: 不过滤 entry_kind, 不过滤 status
-- MIN(provider_id) 是确定性的聚合取法, 近似 flush 的「首值」语义
INSERT INTO receipt_stats_daily (
  date_utc, virtual_model_name, subscription_id, real_model_name, provider_id,
  request_count, input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens
)
SELECT
  (timestamp / 86400000) * 86400000,
  virtual_model_name,
  subscription_id,
  real_model_name,
  MIN(provider_id),
  COUNT(*),
  COALESCE(SUM(upstream_input_tokens), 0),
  COALESCE(SUM(upstream_output_tokens), 0),
  COALESCE(SUM(upstream_cache_creation), 0),
  COALESCE(SUM(upstream_cache_read), 0)
FROM requests
GROUP BY (timestamp / 86400000) * 86400000, virtual_model_name, subscription_id, real_model_name;
