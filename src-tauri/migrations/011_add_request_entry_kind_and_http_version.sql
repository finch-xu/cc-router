-- 请求入口端点 + 下游 HTTP 协议版本
-- entry_kind: "messages" (POST /v1/messages, Anthropic 原生) / "responses" (POST /v1/responses, OpenAI 兼容)
-- downstream_http_version: "HTTP/1.1" / "HTTP/2.0" (CC ↔ cc-router 这一段, 反映 v2.7 ALPN 是否生效)
-- 两列都允许 NULL: 老日志条目无此字段, 前端展示 "—"
ALTER TABLE requests ADD COLUMN entry_kind TEXT;
ALTER TABLE requests ADD COLUMN downstream_http_version TEXT;
