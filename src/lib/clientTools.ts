/**
 * 客户端工具元数据 (与 Rust 侧 `proxy::client_fingerprint::SUPPORTED_TOOLS` 手工同步).
 *
 * 设计与 ProviderIcon 一致: lobe-icons 子路径 import 拿 brand logo, 没有的兜底 lucide.
 * 增删 client tool 时必须三处同时改:
 * 1. Rust SUPPORTED_TOOLS + classify 规则
 * 2. types.ts::ClientToolId union
 * 3. 本文件 CLIENT_TOOLS 数组
 */
import type { ComponentType } from "react";
import { Bot, Box } from "lucide-react";
import Anthropic from "@lobehub/icons/es/Anthropic";
import Claude from "@lobehub/icons/es/Claude";
import ClaudeCode from "@lobehub/icons/es/ClaudeCode";
import Codex from "@lobehub/icons/es/Codex";
import Cursor from "@lobehub/icons/es/Cursor";
import OpenCode from "@lobehub/icons/es/OpenCode";
import type { ClientToolId } from "@/types";

type IconVariant = ComponentType<{ size?: number | string }>;

export interface ClientToolMeta {
  id: ClientToolId;
  /** i18n key, 形如 "clientTool.claudeCode" */
  i18nKey: string;
  icon: IconVariant;
}

export const CLIENT_TOOLS: ClientToolMeta[] = [
  { id: "claude-code", i18nKey: "clientTool.claudeCode", icon: ClaudeCode as IconVariant },
  { id: "claude-desktop", i18nKey: "clientTool.claudeDesktop", icon: Claude as IconVariant },
  { id: "codex-cli", i18nKey: "clientTool.codexCli", icon: Codex as IconVariant },
  { id: "cc-router", i18nKey: "clientTool.ccRouter", icon: Box },
  { id: "zed", i18nKey: "clientTool.zed", icon: Box },
  { id: "cursor", i18nKey: "clientTool.cursor", icon: Cursor as IconVariant },
  { id: "opencode", i18nKey: "clientTool.opencode", icon: OpenCode as IconVariant },
  { id: "anthropic-sdk-python", i18nKey: "clientTool.anthropicSdkPython", icon: Anthropic as IconVariant },
  { id: "anthropic-sdk-js", i18nKey: "clientTool.anthropicSdkJs", icon: Anthropic as IconVariant },
];

export const CLIENT_TOOLS_BY_ID: Record<ClientToolId, ClientToolMeta> = Object.fromEntries(
  CLIENT_TOOLS.map((t) => [t.id, t]),
) as Record<ClientToolId, ClientToolMeta>;

/** 未识别时的兜底 icon (前端表格展示 "unk" 时用) */
export const UNKNOWN_CLIENT_ICON: IconVariant = Bot;
