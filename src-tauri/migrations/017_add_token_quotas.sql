-- 订阅 token 限额配置 (JSON, 形如 {"daily":5000000,"weekly":null,...}); '{}' = 未设限
ALTER TABLE subscriptions ADD COLUMN token_quotas TEXT NOT NULL DEFAULT '{}';

-- 每订阅每周期一行的用量快照 (内存为真值, 这里只做重启恢复)
-- period 取值 daily / weekly / monthly / total
-- period_start_ms 为该周期起点 (本地时区换算成 Unix ms), total 为上次重置时刻
CREATE TABLE subscription_quota_usage (
  subscription_id       TEXT    NOT NULL,
  period                TEXT    NOT NULL,
  period_start_ms       INTEGER NOT NULL,
  input_tokens          INTEGER NOT NULL DEFAULT 0,
  output_tokens         INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens     INTEGER NOT NULL DEFAULT 0,
  updated_at_ms         INTEGER NOT NULL,
  PRIMARY KEY (subscription_id, period)
);
