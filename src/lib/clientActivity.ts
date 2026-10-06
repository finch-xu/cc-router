/**
 * Live Routing「客户端接入」的被动流量检测: 把后端按 client_tool 聚合的结果,
 * 归并成 4 个分组 (Claude / Codex / OpenCode / Others) 的活跃状态 + 卡片
 * 表格分组 —— 与路由图左侧 CLIENT 便签一一对应, 组名也从同一份定义取。
 *
 * 判定是「最近有没有真的发过请求」(零配置), 不做在线/离线判定 —— cc-router
 * 只能看到流量, 看不到客户端是否打开。日志受 `log_retention_days` 清理,
 * 保留期即天然检测窗口。
 */
import { CLIENT_TOOLS } from "@/lib/clientTools";
import type { ClientActivityDto, ClientToolId } from "@/types";

/** 与 RouteFlowDiagram 四张客户端便签一一对应 */
export type ClientGroupKey = "claude" | "codex" | "opencode" | "others";

export interface ClientGroupDef {
  key: ClientGroupKey;
  /** 组名与路由图 CLIENT 便签共用同一串 (便签不走 i18n, 表格也不走; 改这里全跟着变) */
  label: string;
  /** 组内已知 client tool; null = 兜底组 (所有不在前三组的已知 id + 未识别流量) */
  toolIds: ClientToolId[] | null;
}

/**
 * 分组定义, 数组顺序 = 卡片表格的展示顺序。
 *
 * 组名取「产品系列」而非某个具体工具: Anthropic 系一张便签代表一类入口, 底下既有
 * Claude Code, 也有 Claude 桌面端和两个官方 SDK —— 叫 "Claude Code" 名不副实, 且与
 * 同级的 Codex / OpenCode (同样是系列名) 不对称。具体是哪个工具由展开后的成员行
 * (ClientToolBadge) 表达, 层级正好是 系列 > 工具。
 *
 * 增删 client tool 时除「三处同步」外, 新 id 会自动落进 others, 需要进前三组才改这里。
 */
export const CLIENT_GROUPS: ClientGroupDef[] = [
  {
    key: "claude",
    label: "Claude",
    toolIds: ["claude-code", "claude-desktop", "anthropic-sdk-python", "anthropic-sdk-js"],
  },
  { key: "codex", label: "Codex", toolIds: ["codex-cli", "codex-desktop"] },
  { key: "opencode", label: "OpenCode", toolIds: ["opencode"] },
  { key: "others", label: "Others", toolIds: null },
];

/** 组名查表: 路由图便签与「客户端接入」卡都从这里取, 保证两处永远一致 */
export const CLIENT_GROUP_LABEL: Record<ClientGroupKey, string> = Object.fromEntries(
  CLIENT_GROUPS.map((g) => [g.key, g.label]),
) as Record<ClientGroupKey, string>;

/** 已知 id → 分组; 不在 CLIENT_TOOLS 的 id (理论上不可能, 三处同步锁住) 归 "others" */
const GROUP_BY_ID = new Map<string, ClientGroupKey>(
  CLIENT_GROUPS.flatMap((g) => (g.toolIds ?? []).map((id) => [id, g.key] as const)),
);
const KNOWN_IDS = new Set<string>(CLIENT_TOOLS.map((t) => t.id));

export interface ClientGroupActivity {
  /** 保留期内该组有过请求 (未识别的请求只计 "others") */
  active: boolean;
  /** 组内最近一次请求 (ms epoch); 从未有过为 null */
  lastSeen: number | null;
  /** 组内请求总数 */
  count: number;
  /** 组内 token 总数 (输入 + 输出) */
  tokens: number;
}

export interface ClientActivityRow {
  /** 已知 client tool; undefined = 未识别行 */
  toolId?: ClientToolId;
  connected: boolean;
  /** 未接入的已知客户端为 0 */
  count: number;
  /** 输入 + 输出 token; 未接入为 0 */
  tokens: number;
  lastSeen: number | null;
}

/** 卡片表格的一个分组: 组级聚合 + 展开后的成员行 */
export interface ClientGroupSection extends ClientGroupActivity {
  key: ClientGroupKey;
  /** 已接入 (最近在前) → 未接入 (CLIENT_TOOLS 顺序) → 未识别行 (仅 others 组末尾) */
  rows: ClientActivityRow[];
}

export interface ClientActivitySummary {
  /** 保留期内有任何请求。false = 卡片空态文案 */
  hasAny: boolean;
  /** 按组 key 索引 (路由图便签亮/灰用) */
  groups: Record<ClientGroupKey, ClientGroupActivity>;
  /** 卡片表格: 4 个分组, 顺序同 CLIENT_GROUPS */
  table: ClientGroupSection[];
}

const emptyGroup = (): ClientGroupActivity => ({
  active: false,
  lastSeen: null,
  count: 0,
  tokens: 0,
});

/**
 * 汇总后端聚合结果。`data === undefined` (加载中/失败) 返回 null ——
 * 调用方据此保持便签原样, 避免加载瞬间全部变灰的闪烁。
 */
export function summarizeActivity(
  data: ClientActivityDto[] | undefined,
): ClientActivitySummary | null {
  if (!data) return null;

  const groups: Record<ClientGroupKey, ClientGroupActivity> = {
    claude: emptyGroup(),
    codex: emptyGroup(),
    opencode: emptyGroup(),
    others: emptyGroup(),
  };
  const connectedById = new Map<ClientToolId, ClientActivityRow>();
  let unknownRow: ClientActivityRow | undefined;

  for (const r of data) {
    const id = r.client_tool;
    // 未识别桶 (UA 认不出 / 迁移 009 前的老日志) 只计 others
    const g = groups[id ? (GROUP_BY_ID.get(id) ?? "others") : "others"];
    g.active = true;
    g.count += r.request_count;
    g.tokens += r.total_tokens;
    g.lastSeen = g.lastSeen === null ? r.last_seen : Math.max(g.lastSeen, r.last_seen);

    if (!id) {
      unknownRow = {
        connected: true,
        count: r.request_count,
        tokens: r.total_tokens,
        lastSeen: r.last_seen,
      };
    } else if (KNOWN_IDS.has(id)) {
      connectedById.set(id as ClientToolId, {
        toolId: id as ClientToolId,
        connected: true,
        count: r.request_count,
        tokens: r.total_tokens,
        lastSeen: r.last_seen,
      });
    }
    // 不在 CLIENT_TOOLS 的 id 理论不可能; 真出现时上面已计入 others 活跃, 这里不建行
  }

  const table = CLIENT_GROUPS.map((def) => {
    // others 组成员 = 所有不在前三组的已知 id, 保持 CLIENT_TOOLS 顺序
    const memberIds =
      def.toolIds ?? CLIENT_TOOLS.filter((t) => !GROUP_BY_ID.has(t.id)).map((t) => t.id);
    const connectedRows = memberIds
      .map((id) => connectedById.get(id))
      .filter((r): r is ClientActivityRow => r !== undefined)
      .sort((a, b) => (b.lastSeen ?? 0) - (a.lastSeen ?? 0));
    const notConnectedRows: ClientActivityRow[] = memberIds
      .filter((id) => !connectedById.has(id))
      .map((id) => ({ toolId: id, connected: false, count: 0, tokens: 0, lastSeen: null }));
    const rows =
      def.key === "others" && unknownRow
        ? [...connectedRows, ...notConnectedRows, unknownRow]
        : [...connectedRows, ...notConnectedRows];
    return { key: def.key, ...groups[def.key], rows };
  });

  return { hasAny: data.length > 0, groups, table };
}
