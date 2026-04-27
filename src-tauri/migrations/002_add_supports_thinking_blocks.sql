-- v2: 给订阅加 supports_thinking_blocks 能力快照列
-- 默认 0 (false), 创建订阅时由代码从 provider yaml 拷贝默认值
ALTER TABLE subscriptions ADD COLUMN supports_thinking_blocks INTEGER NOT NULL DEFAULT 0;
