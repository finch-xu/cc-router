-- cc-router 初始化 schema (设计稿 §10.3 + §9.1)

CREATE TABLE subscriptions (
  id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL,
  endpoint_id TEXT NOT NULL,
  display_name TEXT NOT NULL,
  api_key TEXT NOT NULL DEFAULT '',  -- 明文存储, 类比 Claude Code 的 settings.json 做法
  model_slot_opus TEXT NOT NULL,
  model_slot_sonnet TEXT NOT NULL,
  model_slot_haiku TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  is_auth_failed INTEGER NOT NULL DEFAULT 0,
  last_error_message TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE virtual_model_bindings (
  virtual_model_name TEXT NOT NULL,
  position INTEGER NOT NULL,
  subscription_id TEXT NOT NULL,
  PRIMARY KEY (virtual_model_name, position),
  FOREIGN KEY (subscription_id) REFERENCES subscriptions(id) ON DELETE CASCADE
);

CREATE INDEX idx_virtual_model_bindings_sub ON virtual_model_bindings(subscription_id);

CREATE TABLE virtual_model_config (
  virtual_model_name TEXT PRIMARY KEY,
  mode TEXT NOT NULL DEFAULT 'sequential'
);

CREATE TABLE model_list_cache (
  subscription_id TEXT NOT NULL,
  endpoint_id TEXT NOT NULL,
  fetched_at INTEGER NOT NULL,
  models_json TEXT NOT NULL,
  PRIMARY KEY (subscription_id, endpoint_id)
);

CREATE TABLE requests (
  id TEXT PRIMARY KEY,
  timestamp INTEGER NOT NULL,
  virtual_model_name TEXT NOT NULL,
  subscription_id TEXT NOT NULL,
  provider_id TEXT NOT NULL,
  endpoint_id TEXT NOT NULL,
  real_model_name TEXT NOT NULL,
  response_model_name TEXT,
  is_streaming INTEGER NOT NULL,
  status TEXT NOT NULL,
  http_status INTEGER,
  ttft_ms INTEGER,
  total_latency_ms INTEGER,
  upstream_input_tokens INTEGER,
  upstream_output_tokens INTEGER,
  upstream_cache_creation INTEGER,
  upstream_cache_read INTEGER,
  retry_count INTEGER NOT NULL DEFAULT 0,
  error_message TEXT
);

CREATE INDEX idx_timestamp ON requests(timestamp);
CREATE INDEX idx_subscription ON requests(subscription_id);
CREATE INDEX idx_virtual_model_ts ON requests(virtual_model_name, timestamp);
CREATE INDEX idx_status ON requests(status);

CREATE TABLE onboarding (
  id INTEGER PRIMARY KEY,
  completed INTEGER NOT NULL DEFAULT 0
);
