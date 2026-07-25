-- v13: 给 subscriptions 表加每槽位 reasoning effort 覆盖列.
-- slot_efforts: JSON, 形如 {"fable":"max","opus":"high"} — 只存用户手动固定过的槽位.
-- 字段缺失 = auto, 即透传 Claude Code 请求里携带的 effort. 空对象 '{}' = 全部 auto.
-- 取值 low|medium|high|xhigh|max, 由 commands 层 validate_slot_efforts 校验.
-- 选 JSON 单列而非 4 个 TEXT 列: 对齐 oauth_metadata 先例, 将来加第 5 个槽位不必再开 migration.
-- 老订阅拿到默认 '{}' 即全 auto, 行为与本次改动前完全一致, 故无需回填语句.
ALTER TABLE subscriptions ADD COLUMN slot_efforts TEXT NOT NULL DEFAULT '{}';
