-- v3: 错误诊断 + 统一事件流
-- 1) requests 表加上游响应原文截断列, 方便错误时排查上游真实报错内容
ALTER TABLE requests ADD COLUMN upstream_response_body TEXT;

-- 2) 新建 events 表, 承载三类事件:
--    request                    每条请求结束的摘要(详情仍读 requests 表)
--    subscription_state_change  订阅健康状态机转换(Healthy 转 RateLimited 等)
--    system_error               系统级故障(DB flush / yaml 加载 / 端口监听 等)
CREATE TABLE events (
  id TEXT PRIMARY KEY,
  timestamp INTEGER NOT NULL,
  kind TEXT NOT NULL,
  severity TEXT NOT NULL,
  subscription_id TEXT,
  request_id TEXT,
  summary TEXT NOT NULL,
  payload TEXT
);

CREATE INDEX idx_events_timestamp ON events(timestamp DESC);
CREATE INDEX idx_events_kind_ts ON events(kind, timestamp DESC);
CREATE INDEX idx_events_subscription_ts ON events(subscription_id, timestamp DESC);
