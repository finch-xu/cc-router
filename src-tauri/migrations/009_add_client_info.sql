-- 请求来源识别：客户端工具 / 原始 UA / 版本 / 对端 IP
-- client_tool 为 NULL 表示「未识别」(unk), 由 client_fingerprint::identify 决定取值
ALTER TABLE requests ADD COLUMN client_tool TEXT;
ALTER TABLE requests ADD COLUMN client_user_agent TEXT;
ALTER TABLE requests ADD COLUMN client_version TEXT;
ALTER TABLE requests ADD COLUMN client_ip TEXT;
CREATE INDEX IF NOT EXISTS idx_requests_client_tool ON requests(client_tool);
