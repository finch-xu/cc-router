-- System plugins (provider / pricing / rules / notifier): per-plugin enable flag, ordering and user params.
-- No row = the plugin type's default (plugins/config.rs). Themes / receipts never read this table.
CREATE TABLE IF NOT EXISTS plugin_configs (
  plugin_id  TEXT PRIMARY KEY,
  enabled    INTEGER NOT NULL,
  priority   INTEGER NOT NULL DEFAULT 0,
  params     TEXT    NOT NULL DEFAULT '{}',
  updated_at INTEGER NOT NULL
)
