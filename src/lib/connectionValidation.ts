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
