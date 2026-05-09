-- v8: 给 subscriptions 表加 OAuth 凭据字段, 用于 OpenAI Codex (ChatGPT 订阅) provider.
-- auth_type: 'api_key' (现有所有订阅) | 'chatgpt_oauth' (新)
-- oauth_metadata: JSON, 内容形如 {"account_id": "...", "email": "...", "refresh_token": "...", "authenticated_at": <ms>}
-- 仅对 auth_type='chatgpt_oauth' 有效, 其他场景为空 JSON '{}'.

ALTER TABLE subscriptions ADD COLUMN auth_type TEXT NOT NULL DEFAULT 'api_key';
ALTER TABLE subscriptions ADD COLUMN oauth_metadata TEXT NOT NULL DEFAULT '{}';
