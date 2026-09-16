-- request 事件下线 (2026-09-16): 每 attempt 一条、前端无消费者、信息与 requests 表重复
DELETE FROM events WHERE kind = 'request';

-- p95 查询按 total_latency_ms 排序取分位, 之前无索引会全表排序
CREATE INDEX IF NOT EXISTS idx_requests_latency ON requests(total_latency_ms);
