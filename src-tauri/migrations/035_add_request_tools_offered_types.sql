-- 请求 tools[] 里出现过的工具类型 (JSON 数组, 去重排序, 普通工具记 custom; NULL = 请求没带 tools 或老数据)。
-- 只有协议里的类型标识, 不含工具名或 schema; 供规则插件试运行判断 tool_type 条件。
ALTER TABLE requests ADD COLUMN tools_offered_types TEXT;
