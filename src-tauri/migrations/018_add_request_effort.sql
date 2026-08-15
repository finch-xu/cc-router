-- 请求日志的思考强度三格 (客户端请求 / 实际发往上游 + 来源 / 上游回显)
-- 四列全部可空, 任何一格解析不到就是 NULL, 老日志同样为 NULL
ALTER TABLE requests ADD COLUMN client_effort TEXT;
ALTER TABLE requests ADD COLUMN effective_effort TEXT;
-- effort_source 取值 slot / client / yaml, 未知为 NULL
ALTER TABLE requests ADD COLUMN effort_source TEXT;
-- upstream_effort 仅 OpenAI Responses 系上游会回显, 其余 provider 恒 NULL
ALTER TABLE requests ADD COLUMN upstream_effort TEXT;
