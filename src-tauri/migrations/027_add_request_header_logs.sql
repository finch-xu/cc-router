-- 请求日志记录三组请求头 (收到的 / 发往上游的 / 上游返回的), JSON 数组, 凭据已脱敏
ALTER TABLE requests ADD COLUMN client_request_headers TEXT;
ALTER TABLE requests ADD COLUMN upstream_request_headers TEXT;
ALTER TABLE requests ADD COLUMN upstream_response_headers TEXT;
