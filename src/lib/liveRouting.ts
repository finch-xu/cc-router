import type { VirtualModelName } from "@/types";

/* 实时路由页 (经典 / 手绘两版) 共用的静态数据 */

/**
 * 客户端可以填的模型名 → 虚拟模型。
 * 事实来源是 Rust 侧 `virtual_model/model.rs::parse`, 改那边必须同步这里。
 * `*` 表示模糊匹配; `anthropic/` `openai/` 前缀对所有别名通用, 抽到脚注里说明。
 */
export const CLIENT_ALIASES: Record<VirtualModelName, string[]> = {
  "model-fable": ["model-fable", "claude-fable*", "gpt-*-astra"],
  "model-opus": ["model-opus", "claude-opus*", "gpt-*-sol"],
  "model-sonnet": ["model-sonnet", "claude-sonnet*", "gpt-*-terra"],
  "model-haiku": ["model-haiku", "claude-haiku*", "gpt-*-luna"],
  "model-fallback": [],
  "model-jev": ["model-jev"],
};

/** 照抄 src-tauri/src/proxy/server.rs::build_router —— 唯一事实来源 */
export const API_ROUTES: { method: string; path: string; descKey: string }[] = [
  { method: "POST", path: "/v1/messages", descKey: "liveRouting.api.messages" },
  { method: "POST", path: "/v1/responses", descKey: "liveRouting.api.responses" },
  { method: "POST", path: "/v1/chat/completions", descKey: "liveRouting.api.chatCompletions" },
  { method: "POST", path: "/v1/systemone", descKey: "liveRouting.api.systemone" },
  { method: "GET", path: "/v1/models", descKey: "liveRouting.api.models" },
  { method: "GET", path: "/health", descKey: "liveRouting.api.health" },
];
