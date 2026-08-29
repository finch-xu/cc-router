-- 透传订阅的「透传客户端请求头」开关 (0 = 关闭, 与历史行为一致)
-- 打开后 Anthropic 透传路径按内置白名单转发客户端头 (proxy/forward.rs)
ALTER TABLE subscriptions ADD COLUMN forward_client_headers INTEGER NOT NULL DEFAULT 0;
