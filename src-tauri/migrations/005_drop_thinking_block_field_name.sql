-- v5: 移除 thinking_block_field_name 列。重建表方案以兼容老版本 SQLite (无 DROP COLUMN)。

PRAGMA foreign_keys=OFF;

CREATE TABLE subscriptions_new (
  id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL,
  endpoint_id TEXT NOT NULL,
  display_name TEXT NOT NULL,
  api_key TEXT NOT NULL DEFAULT '',
  model_slot_opus TEXT NOT NULL,
  model_slot_sonnet TEXT NOT NULL,
  model_slot_haiku TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  is_auth_failed INTEGER NOT NULL DEFAULT 0,
  last_error_message TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  base_url TEXT NOT NULL,
  messages_path TEXT NOT NULL,
  auth_header_name TEXT NOT NULL,
  auth_header_format TEXT NOT NULL,
  required_headers TEXT NOT NULL DEFAULT '{}',
  forward_headers TEXT NOT NULL DEFAULT '[]',
  model_discovery TEXT NOT NULL DEFAULT '{}',
  provider_display_name TEXT NOT NULL,
  provider_icon TEXT NOT NULL DEFAULT '',
  is_user_defined INTEGER NOT NULL DEFAULT 0,
  supports_thinking_blocks INTEGER NOT NULL DEFAULT 0
);

INSERT INTO subscriptions_new
SELECT
  id, provider_id, endpoint_id, display_name, api_key,
  model_slot_opus, model_slot_sonnet, model_slot_haiku,
  enabled, is_auth_failed, last_error_message, created_at, updated_at,
  base_url, messages_path, auth_header_name, auth_header_format,
  required_headers, forward_headers, model_discovery,
  provider_display_name, provider_icon, is_user_defined,
  supports_thinking_blocks
FROM subscriptions;

DROP TABLE subscriptions;
ALTER TABLE subscriptions_new RENAME TO subscriptions;

PRAGMA foreign_keys=ON;
