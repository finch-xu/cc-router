-- 订阅级兜底模型槽位: fallback 虚拟模型命中该订阅时, 非空则把请求 model 改写为此值,
-- 空串 = 未配置 = 透传原始 model (老订阅默认行为不变, 无需回填).
ALTER TABLE subscriptions ADD COLUMN model_slot_fallback TEXT NOT NULL DEFAULT '';
