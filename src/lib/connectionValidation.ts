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

export type HeaderPolicyNameErrorKey =
  | "validation.headerPolicyNameEmpty"
  | "validation.headerPolicyNameStar"
  | "validation.headerPolicyNameInvalid";

/** HTTP token 字符 (RFC 9110), 与后端 reqwest HeaderName 解析口径一致。 */
const HEADER_TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

/** 透传策略名单校验, 规则同后端 `HeaderPolicy::validate`: `*` 只能在末尾表示前缀。 */
export function validateHeaderPolicyNames(names: string[]): HeaderPolicyNameErrorKey | null {
  for (const raw of names) {
    const stem = raw.endsWith("*") ? raw.slice(0, -1) : raw;
    if (stem === "") return "validation.headerPolicyNameEmpty";
    if (stem.includes("*")) return "validation.headerPolicyNameStar";
    if (!HEADER_TOKEN.test(stem)) return "validation.headerPolicyNameInvalid";
  }
  return null;
}
