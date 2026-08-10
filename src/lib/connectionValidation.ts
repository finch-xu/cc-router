/** 自定义订阅的 base_url / messages_path 字段校验。后端 commands/subscriptions.rs 也有同款检查。
 *  返回 i18n key(由调用方 t() 化),而非已翻译字符串。 */
export type ConnectionErrorKey =
  | "validation.baseUrl"
  | "validation.messagesPath";

export function validateConnection(input: {
  base_url: string;
  messages_path: string;
}): ConnectionErrorKey | null {
  if (!input.base_url.startsWith("http://") && !input.base_url.startsWith("https://")) {
    return "validation.baseUrl";
  }
  if (!input.messages_path.startsWith("/")) {
    return "validation.messagesPath";
  }
  return null;
}

export type RequiredHeadersErrorKey =
  | "validation.requiredHeaderIncomplete"
  | "validation.requiredHeaderDuplicate";

/** 只拦「半填行」与「大小写不敏感重名」两类会静默丢数据的错误
 *  (Object.fromEntries 后者覆盖前者, 重名不在前端拦、到后端前就已合并),
 *  其余校验交后端中文报错。 */
export function validateRequiredHeaders(
  rows: Array<{ key: string; value: string }>,
): RequiredHeadersErrorKey | null {
  const seen = new Set<string>();
  for (const r of rows) {
    if (r.key === "" || r.value === "") return "validation.requiredHeaderIncomplete";
    const lower = r.key.toLowerCase();
    if (seen.has(lower)) return "validation.requiredHeaderDuplicate";
    seen.add(lower);
  }
  return null;
}
