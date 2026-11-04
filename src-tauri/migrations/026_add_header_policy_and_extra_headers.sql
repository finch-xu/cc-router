-- 请求头透传策略 (取代 forward_client_headers 布尔开关, 旧列保留不再读取)
-- 与用户自定义出站头 (内置订阅也可编辑, yaml 重拍不覆盖)
ALTER TABLE subscriptions ADD COLUMN header_policy TEXT NOT NULL DEFAULT '{"mode":"none","names":[]}';
ALTER TABLE subscriptions ADD COLUMN extra_headers TEXT NOT NULL DEFAULT '{}';
UPDATE subscriptions SET header_policy = '{"mode":"claude_code","names":[]}' WHERE forward_client_headers = 1;
