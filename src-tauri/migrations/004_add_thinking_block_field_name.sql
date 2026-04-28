-- v4: 给订阅加 thinking 块内部承载字段名 capability 列
-- Anthropic 标准用 'thinking', DeepSeek 兼容层用 'think'
-- pipeline 在请求侧把 thinking 重命名为目标字段, 响应侧反向重命名回 thinking
-- 让 CC 始终以 Anthropic 标准格式收发, 对协议方言无感知
ALTER TABLE subscriptions ADD COLUMN thinking_block_field_name TEXT NOT NULL DEFAULT 'thinking';

-- 把已存在的 deepseek 订阅一次性修复:
-- 1. supports_thinking_blocks=1 让 pipeline 走方言翻译路径而不是 strip
-- 2. thinking_block_field_name='think' 触发实际翻译
-- 这是对 v2 引入开关后, deepseek 老订阅 supports_thinking_blocks=0 默认值的纠正
UPDATE subscriptions
   SET supports_thinking_blocks = 1,
       thinking_block_field_name = 'think'
 WHERE provider_id = 'deepseek';
