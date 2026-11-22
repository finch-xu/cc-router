-- 规则插件对本次请求做过的改道 / 改写记录 (JSON 数组, NULL = 未命中任何规则插件)。
ALTER TABLE requests ADD COLUMN plugins_applied TEXT;
