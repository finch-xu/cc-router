/** 自定义订阅的 base_url / messages_path 字段校验。后端 commands/subscriptions.rs 也有同款检查。 */
export function validateConnection(input: {
  base_url: string;
  messages_path: string;
}): string | null {
  if (!input.base_url.startsWith("http://") && !input.base_url.startsWith("https://")) {
    return "base URL 必须以 http:// 或 https:// 开头";
  }
  if (!input.messages_path.startsWith("/")) {
    return "messages path 必须以 / 开头";
  }
  return null;
}
