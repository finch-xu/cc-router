-- 日聚合表 key 从「UTC 日 0 点 ms (date_utc)」改为「本地日历日字符串 (day, YYYY-MM-DD)」。
-- 动机: 统计页 / 小票的「今天」对东八区用户曾从早 8 点算起, 限额早已按本地日历切桶, 统计对齐。
-- 存日历日而非「本地午夜的 UTC 瞬时」: 瞬时 -> 本地日永远无歧义, DST / 换时区不会让同一天分裂成两行,
-- SQL date(...,'localtime') / Rust chrono::Local / 前端本地 getter 三端天然一致。
--
-- 整段用显式事务: run_migrations 在同一连接上逐条执行且不开事务, 若 COMMIT 后崩溃再重跑会因
-- date_utc 列已不存在而卡死, 所以把版本号写入也放进事务, 原子提交后再启动不会重跑本文件。
-- 临时表按 db/mod.rs 的约定带版本号 (_v19_new), 与 v5 的 subscriptions_new 自愈逻辑区分。

BEGIN;

CREATE TABLE request_stats_daily_v19_new (
  day                TEXT    NOT NULL,
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

  PRIMARY KEY (day, virtual_model_name, subscription_id)
);

-- 1) 从 requests 原始表按本地日重建, 口径与 request_log::flush_batch 完全一致:
--    status 三分计数 / 四类 token unwrap_or(0) / latency 与 ttft 仅非 NULL 计入 sum+count / retry 累加。
--    MIN(provider_id) 是确定性的聚合取法, 近似 flush 的「首值」语义 (同 015)。
INSERT INTO request_stats_daily_v19_new (
  day, virtual_model_name, subscription_id, provider_id,
  request_count, success_count, error_count, timeout_count,
  input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
  total_duration_ms_sum, total_duration_ms_count, ttft_ms_sum, ttft_ms_count,
  retry_count_sum
)
SELECT
  date(timestamp / 1000, 'unixepoch', 'localtime'),
  virtual_model_name,
  subscription_id,
  MIN(provider_id),
  COUNT(*),
  SUM(status = 'success'),
  SUM(status = 'error'),
  SUM(status = 'timeout'),
  COALESCE(SUM(upstream_input_tokens), 0),
  COALESCE(SUM(upstream_output_tokens), 0),
  COALESCE(SUM(upstream_cache_creation), 0),
  COALESCE(SUM(upstream_cache_read), 0),
  COALESCE(SUM(total_latency_ms), 0),
  COUNT(total_latency_ms),
  COALESCE(SUM(ttft_ms), 0),
  COUNT(ttft_ms),
  COALESCE(SUM(retry_count), 0)
FROM requests
GROUP BY 1, 2, 3;

-- 2) requests 已被 log_retention_days 清理过的老用户: 早于原始日志覆盖范围的旧 UTC 日行,
--    以 UTC 日期字符串近似搬入 (对东八区用户 UTC 日 0 点 = 当天早 8 点, 日期字符串本身不变)。
--    边界: 只搬「整行完全早于首条 requests 所在 UTC 日」的行 (date_utc + 1 天 <= 首条的 UTC 日 0 点),
--    跨界那一个 UTC 日的旧行整体丢弃, 避免与按本地日回填的行双计, 接受最多 1 天的边界误差。
--    requests 为空时 MIN 为 NULL, COALESCE 成 i64::MAX 让所有旧行都搬入。
INSERT OR IGNORE INTO request_stats_daily_v19_new (
  day, virtual_model_name, subscription_id, provider_id,
  request_count, success_count, error_count, timeout_count,
  input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
  total_duration_ms_sum, total_duration_ms_count, ttft_ms_sum, ttft_ms_count,
  retry_count_sum
)
SELECT
  date(date_utc / 1000, 'unixepoch'),
  virtual_model_name, subscription_id, provider_id,
  request_count, success_count, error_count, timeout_count,
  input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
  total_duration_ms_sum, total_duration_ms_count, ttft_ms_sum, ttft_ms_count,
  retry_count_sum
FROM request_stats_daily
WHERE date_utc + 86400000 <= (
  SELECT COALESCE(MIN(timestamp) - (MIN(timestamp) % 86400000), 9223372036854775807) FROM requests
);

DROP TABLE request_stats_daily;
ALTER TABLE request_stats_daily_v19_new RENAME TO request_stats_daily;
CREATE INDEX idx_stats_daily_day ON request_stats_daily(day DESC);

-- receipt_stats_daily 同上范式, 多 real_model_name 一维, 只有小票需要的 5 个计数列 (同 015)。
CREATE TABLE receipt_stats_daily_v19_new (
  day                TEXT    NOT NULL,
  virtual_model_name TEXT    NOT NULL,
  subscription_id    TEXT    NOT NULL,
  real_model_name    TEXT    NOT NULL,
  provider_id        TEXT    NOT NULL,

  request_count         INTEGER NOT NULL DEFAULT 0,
  input_tokens          INTEGER NOT NULL DEFAULT 0,
  output_tokens         INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens     INTEGER NOT NULL DEFAULT 0,

  PRIMARY KEY (day, virtual_model_name, subscription_id, real_model_name)
);

INSERT INTO receipt_stats_daily_v19_new (
  day, virtual_model_name, subscription_id, real_model_name, provider_id,
  request_count, input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens
)
SELECT
  date(timestamp / 1000, 'unixepoch', 'localtime'),
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
GROUP BY 1, 2, 3, 4;

INSERT OR IGNORE INTO receipt_stats_daily_v19_new (
  day, virtual_model_name, subscription_id, real_model_name, provider_id,
  request_count, input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens
)
SELECT
  date(date_utc / 1000, 'unixepoch'),
  virtual_model_name, subscription_id, real_model_name, provider_id,
  request_count, input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens
FROM receipt_stats_daily
WHERE date_utc + 86400000 <= (
  SELECT COALESCE(MIN(timestamp) - (MIN(timestamp) % 86400000), 9223372036854775807) FROM requests
);

DROP TABLE receipt_stats_daily;
ALTER TABLE receipt_stats_daily_v19_new RENAME TO receipt_stats_daily;
CREATE INDEX idx_receipt_stats_daily_day ON receipt_stats_daily(day DESC);

-- 版本号随本事务一起提交 (run_migrations 之后的 INSERT OR IGNORE 会成为空操作)。
INSERT OR IGNORE INTO _schema_version (version, applied_at)
VALUES (19, CAST(strftime('%s', 'now') AS INTEGER) * 1000);

COMMIT;
