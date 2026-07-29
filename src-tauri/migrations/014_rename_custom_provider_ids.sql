-- 自定义 provider 的来源标记从 __custom__ 系双下划线格式改为 kebab-case 短格式
-- (subscription/model.rs 的 CUSTOM_*_SOURCE_MARKER 常量同步改值)
-- 覆盖 4 张表 6 列: subscriptions.provider_id/endpoint_id, requests.provider_id/endpoint_id,
-- request_stats_daily.provider_id (永久保留表), model_list_cache.endpoint_id
-- model_list_cache PK 为 (subscription_id, endpoint_id), subscription_id 唯一, 改值不会撞 PK

UPDATE subscriptions SET provider_id = CASE provider_id
  WHEN '__custom__' THEN 'custom'
  WHEN '__custom_gemini__' THEN 'custom-gemini'
  WHEN '__custom_openai__' THEN 'custom-openai'
  WHEN '__custom_openai_chat__' THEN 'custom-openai-chat'
  WHEN '__custom_gemini_interactions__' THEN 'custom-gemini-interactions'
END
WHERE provider_id IN ('__custom__', '__custom_gemini__', '__custom_openai__', '__custom_openai_chat__', '__custom_gemini_interactions__');

UPDATE subscriptions SET endpoint_id = CASE endpoint_id
  WHEN '__custom__' THEN 'custom'
  WHEN '__custom_gemini__' THEN 'custom-gemini'
  WHEN '__custom_openai__' THEN 'custom-openai'
  WHEN '__custom_openai_chat__' THEN 'custom-openai-chat'
  WHEN '__custom_gemini_interactions__' THEN 'custom-gemini-interactions'
END
WHERE endpoint_id IN ('__custom__', '__custom_gemini__', '__custom_openai__', '__custom_openai_chat__', '__custom_gemini_interactions__');

UPDATE requests SET provider_id = CASE provider_id
  WHEN '__custom__' THEN 'custom'
  WHEN '__custom_gemini__' THEN 'custom-gemini'
  WHEN '__custom_openai__' THEN 'custom-openai'
  WHEN '__custom_openai_chat__' THEN 'custom-openai-chat'
  WHEN '__custom_gemini_interactions__' THEN 'custom-gemini-interactions'
END
WHERE provider_id IN ('__custom__', '__custom_gemini__', '__custom_openai__', '__custom_openai_chat__', '__custom_gemini_interactions__');

UPDATE requests SET endpoint_id = CASE endpoint_id
  WHEN '__custom__' THEN 'custom'
  WHEN '__custom_gemini__' THEN 'custom-gemini'
  WHEN '__custom_openai__' THEN 'custom-openai'
  WHEN '__custom_openai_chat__' THEN 'custom-openai-chat'
  WHEN '__custom_gemini_interactions__' THEN 'custom-gemini-interactions'
END
WHERE endpoint_id IN ('__custom__', '__custom_gemini__', '__custom_openai__', '__custom_openai_chat__', '__custom_gemini_interactions__');

UPDATE request_stats_daily SET provider_id = CASE provider_id
  WHEN '__custom__' THEN 'custom'
  WHEN '__custom_gemini__' THEN 'custom-gemini'
  WHEN '__custom_openai__' THEN 'custom-openai'
  WHEN '__custom_openai_chat__' THEN 'custom-openai-chat'
  WHEN '__custom_gemini_interactions__' THEN 'custom-gemini-interactions'
END
WHERE provider_id IN ('__custom__', '__custom_gemini__', '__custom_openai__', '__custom_openai_chat__', '__custom_gemini_interactions__');

UPDATE model_list_cache SET endpoint_id = CASE endpoint_id
  WHEN '__custom__' THEN 'custom'
  WHEN '__custom_gemini__' THEN 'custom-gemini'
  WHEN '__custom_openai__' THEN 'custom-openai'
  WHEN '__custom_openai_chat__' THEN 'custom-openai-chat'
  WHEN '__custom_gemini_interactions__' THEN 'custom-gemini-interactions'
END
WHERE endpoint_id IN ('__custom__', '__custom_gemini__', '__custom_openai__', '__custom_openai_chat__', '__custom_gemini_interactions__');
