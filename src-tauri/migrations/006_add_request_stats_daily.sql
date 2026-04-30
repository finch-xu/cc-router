-- 按 (date_utc, virtual_model_name, subscription_id) 三维聚合的统计表。
-- date_utc = 当天 UTC 0 点的 Unix ms (timestamp / 86_400_000 * 86_400_000)。
-- 跟 requests 表共生: flush 写日志的同事务里 UPSERT 这张表;
-- requests 表受 log_retention_days 清理, 但本表永久保留, 让历史用量统计不受清理影响。

CREATE TABLE request_stats_daily (
  date_utc           INTEGER NOT NULL,
  virtual_model_name TEXT    NOT NULL,
  subscription_id    TEXT    NOT NULL,
  provider_id        TEXT    NOT NULL,

  request_count INTEGER NOT NULL DEFAULT 0,
  success_count INTEGER NOT NULL DEFAULT 0,
  error_count   INTEGER NOT NULL DEFAULT 0,
  timeout_count INTEGER NOT NULL DEFAULT 0,

  input_tokens          INTEGER NOT NULL DEFAULT 0,
  output_tokens         INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens     INTEGER NOT NULL DEFAULT 0,

  total_duration_ms_sum   INTEGER NOT NULL DEFAULT 0,
  total_duration_ms_count INTEGER NOT NULL DEFAULT 0,
  ttft_ms_sum             INTEGER NOT NULL DEFAULT 0,
  ttft_ms_count           INTEGER NOT NULL DEFAULT 0,

  retry_count_sum INTEGER NOT NULL DEFAULT 0,

  PRIMARY KEY (date_utc, virtual_model_name, subscription_id)
);

CREATE INDEX idx_stats_daily_date ON request_stats_daily(date_utc DESC);
