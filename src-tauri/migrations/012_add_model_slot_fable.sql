-- 新增第 5 个虚拟模型 fable 的 model slot 列 (对应 ANTHROPIC_DEFAULT_FABLE_MODEL).
-- 存量订阅回填 = opus 槽值: fable 是最强模型, opus 是已有槽里最接近的, 让存量订阅
-- 立刻能处理 model-fable 请求而不至于因空 model 被上游拒. 用户之后可改成真正的 fable 级模型.
ALTER TABLE subscriptions ADD COLUMN model_slot_fable TEXT NOT NULL DEFAULT '';
UPDATE subscriptions SET model_slot_fable = model_slot_opus WHERE model_slot_fable = '';
