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
import { Bot, Box, Code, Globe, Terminal } from "lucide-react";
import Anthropic from "@lobehub/icons/es/Anthropic";
import Claude from "@lobehub/icons/es/Claude";
import ClaudeCode from "@lobehub/icons/es/ClaudeCode";
import Codex from "@lobehub/icons/es/Codex";
import Cursor from "@lobehub/icons/es/Cursor";
import OpenAI from "@lobehub/icons/es/OpenAI";
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
  { id: "codex-desktop", i18nKey: "clientTool.codexDesktop", icon: Codex as IconVariant },
  { id: "cc-router", i18nKey: "clientTool.ccRouter", icon: Box },
  { id: "zed", i18nKey: "clientTool.zed", icon: Box },
  { id: "cursor", i18nKey: "clientTool.cursor", icon: Cursor as IconVariant },
  { id: "opencode", i18nKey: "clientTool.opencode", icon: OpenCode as IconVariant },
  { id: "anthropic-sdk-python", i18nKey: "clientTool.anthropicSdkPython", icon: Anthropic as IconVariant },
  { id: "anthropic-sdk-js", i18nKey: "clientTool.anthropicSdkJs", icon: Anthropic as IconVariant },
  { id: "openai-sdk-python", i18nKey: "clientTool.openaiSdkPython", icon: OpenAI as IconVariant },
  { id: "openai-sdk-js", i18nKey: "clientTool.openaiSdkJs", icon: OpenAI as IconVariant },
  { id: "python-requests", i18nKey: "clientTool.pythonRequests", icon: Code },
  { id: "python-httpx", i18nKey: "clientTool.pythonHttpx", icon: Code },
  { id: "axios", i18nKey: "clientTool.axios", icon: Code },
  { id: "node-js", i18nKey: "clientTool.nodeJs", icon: Code },
  { id: "curl", i18nKey: "clientTool.curl", icon: Terminal },
  { id: "browser", i18nKey: "clientTool.browser", icon: Globe },
  { id: "go-net-http", i18nKey: "clientTool.goNetHttp", icon: Code },
  { id: "go-resty", i18nKey: "clientTool.goResty", icon: Code },
  { id: "rust-ureq", i18nKey: "clientTool.rustUreq", icon: Code },
  { id: "rust-reqwest", i18nKey: "clientTool.rustReqwest", icon: Code },
  { id: "rust-hyper", i18nKey: "clientTool.rustHyper", icon: Code },
  { id: "python-urllib", i18nKey: "clientTool.pythonUrllib", icon: Code },
  { id: "python-aiohttp", i18nKey: "clientTool.pythonAiohttp", icon: Code },
  { id: "bun", i18nKey: "clientTool.bun", icon: Code },
  { id: "deno", i18nKey: "clientTool.deno", icon: Code },
  { id: "java-http-client", i18nKey: "clientTool.javaHttpClient", icon: Code },
  { id: "okhttp", i18nKey: "clientTool.okhttp", icon: Code },
  { id: "apache-http-client", i18nKey: "clientTool.apacheHttpClient", icon: Code },
  { id: "powershell", i18nKey: "clientTool.powershell", icon: Terminal },
  { id: "guzzle", i18nKey: "clientTool.guzzle", icon: Code },
  { id: "faraday", i18nKey: "clientTool.faraday", icon: Code },
  { id: "httpie", i18nKey: "clientTool.httpie", icon: Terminal },
  { id: "wget", i18nKey: "clientTool.wget", icon: Terminal },
  { id: "script-go", i18nKey: "clientTool.scriptGo", icon: Code },
  { id: "script-rust", i18nKey: "clientTool.scriptRust", icon: Code },
  { id: "script-python", i18nKey: "clientTool.scriptPython", icon: Code },
  { id: "script-javascript", i18nKey: "clientTool.scriptJavaScript", icon: Code },
  { id: "script-typescript", i18nKey: "clientTool.scriptTypeScript", icon: Code },
  { id: "script-java", i18nKey: "clientTool.scriptJava", icon: Code },
  { id: "script-dotnet", i18nKey: "clientTool.scriptDotnet", icon: Code },
  { id: "script-php", i18nKey: "clientTool.scriptPhp", icon: Code },
  { id: "script-ruby", i18nKey: "clientTool.scriptRuby", icon: Code },
];

export const CLIENT_TOOLS_BY_ID: Record<ClientToolId, ClientToolMeta> = Object.fromEntries(
  CLIENT_TOOLS.map((t) => [t.id, t]),
) as Record<ClientToolId, ClientToolMeta>;

/** 未识别时的兜底 icon (前端表格展示 "unk" 时用) */
export const UNKNOWN_CLIENT_ICON: IconVariant = Bot;
