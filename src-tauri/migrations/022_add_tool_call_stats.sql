-- 工具调用统计 (spec 2026-09-16 §4.1)
-- requests 5 列: 全部可空, 老行 NULL
ALTER TABLE requests ADD COLUMN stop_reason TEXT;
ALTER TABLE requests ADD COLUMN tools_offered_count INTEGER;
ALTER TABLE requests ADD COLUMN tool_result_count INTEGER;
ALTER TABLE requests ADD COLUMN tool_use_count INTEGER;
ALTER TABLE requests ADD COLUMN tool_use_names TEXT;

-- request_stats_daily 3 列: 累加计数
ALTER TABLE request_stats_daily ADD COLUMN tool_use_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE request_stats_daily ADD COLUMN tool_use_request_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE request_stats_daily ADD COLUMN tool_result_count INTEGER NOT NULL DEFAULT 0;

-- 按 (本地日, 客户端, 工具名) 的调用次数, 永久保留, 不受 log_retention_days 影响
-- client_tool 未识别时写 '__unknown__' (与 commands/requests.rs UNKNOWN_SENTINEL 同值)
CREATE TABLE tool_stats_daily (
  day         TEXT    NOT NULL,
  client_tool TEXT    NOT NULL,
  tool_name   TEXT    NOT NULL,
  call_count  INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, client_tool, tool_name)
);
CREATE INDEX idx_tool_stats_daily_day ON tool_stats_daily(day DESC);
