import { ArrowRight, Send, Route, Cloud } from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { useProxyStatus } from "@/hooks/useSettings";
import { useVirtualModels } from "@/hooks/useVirtualModels";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useProviders } from "@/hooks/useProviders";
import type {
  RoutingMode,
  SubscriptionDto,
  VirtualModelDto,
  VirtualModelName,
} from "@/types";

const VM_ORDER: VirtualModelName[] = [
  "model-opus",
  "model-sonnet",
  "model-haiku",
  "model-fallback",
];

const MODE_LABEL: Record<RoutingMode, string> = {
  sequential: "顺序",
  round_robin: "轮询",
};

export function RouteFlowDiagram() {
  const proxy = useProxyStatus();
  const vms = useVirtualModels();
  const subs = useSubscriptions();
  const providers = useProviders();

  const subsMap = new Map<string, SubscriptionDto>(
    subs.data?.map((s) => [s.id, s]) ?? [],
  );
  const providerName = (id: string) =>
    providers.data?.find((p) => p.id === id)?.display_name ?? id;

  const orderedVms: VirtualModelDto[] = VM_ORDER.map((name) =>
    vms.data?.find((v) => v.name === name),
  ).filter((v): v is VirtualModelDto => v !== undefined);

  // 右侧"厂商"节点展示所有被引用的 provider（去重）
  const providersInUse = new Set<string>();
  for (const vm of orderedVms) {
    for (const sid of vm.subscription_ids) {
      const sub = subsMap.get(sid);
      if (sub) providersInUse.add(sub.provider_id);
    }
  }

  return (
    <Card>
      <CardContent className="p-6">
        <div className="mb-4 text-sm font-medium">请求路由</div>

        <div className="flex items-stretch gap-3">
          {/* 左：Claude Code */}
          <FlowNode
            icon={<Send className="h-4 w-4" />}
            title="Claude Code"
            lines={["发起 /v1/messages"]}
          />

          <FlowArrow />

          {/* 中：cc-router（占主要宽度） */}
          <div className="flex flex-1 flex-col rounded-lg border bg-muted/30 p-4">
            <div className="mb-2 flex items-center gap-2 text-sm font-semibold">
              <Route className="h-4 w-4" />
              cc-router
              <span className="font-mono text-xs text-muted-foreground">
                127.0.0.1:{proxy.data?.port ?? "?"}
              </span>
            </div>

            {orderedVms.length === 0 ? (
              <div className="text-xs text-muted-foreground">加载中…</div>
            ) : (
              <div className="divide-y text-xs">
                {orderedVms.map((vm) => (
                  <VmRow
                    key={vm.name}
                    vm={vm}
                    subsMap={subsMap}
                    providerName={providerName}
                  />
                ))}
              </div>
            )}
          </div>

          <FlowArrow />

          {/* 右：厂商池 */}
          <FlowNode
            icon={<Cloud className="h-4 w-4" />}
            title="厂商订阅"
            lines={
              providersInUse.size === 0
                ? ["(未绑定)"]
                : Array.from(providersInUse).map(providerName)
            }
          />
        </div>
      </CardContent>
    </Card>
  );
}

function FlowNode({
  icon,
  title,
  lines,
}: {
  icon: React.ReactNode;
  title: string;
  lines: string[];
}) {
  return (
    <div className="flex w-40 flex-col rounded-lg border bg-card p-4">
      <div className="mb-2 flex items-center gap-2 text-sm font-semibold">
        {icon}
        {title}
      </div>
      <div className="space-y-0.5 text-xs text-muted-foreground">
        {lines.map((l, i) => (
          <div key={i} className="truncate">
            {l}
          </div>
        ))}
      </div>
    </div>
  );
}

function FlowArrow() {
  return (
    <div className="flex items-center text-muted-foreground">
      <ArrowRight className="h-5 w-5" />
    </div>
  );
}

function VmRow({
  vm,
  subsMap,
  providerName,
}: {
  vm: VirtualModelDto;
  subsMap: Map<string, SubscriptionDto>;
  providerName: (id: string) => string;
}) {
  const unboundLabel = "(未绑定)";

  // 目标订阅：按 mode 展示（顺序用 →，轮询用 |）
  const sep = vm.mode === "sequential" ? " → " : " | ";
  const targets =
    vm.subscription_ids.length === 0
      ? unboundLabel
      : vm.subscription_ids
          .map((sid) => {
            const sub = subsMap.get(sid);
            if (!sub) return "?";
            return `${sub.display_name} [${providerName(sub.provider_id)}]`;
          })
          .join(sep);

  return (
    <div className="grid grid-cols-[130px_50px_1fr] items-center gap-2 py-1.5">
      <span className="font-mono text-[11px]">{vm.name}</span>
      <span className="rounded bg-background px-1.5 py-0.5 text-center text-[10px]">
        {MODE_LABEL[vm.mode]}
      </span>
      <span
        className={
          vm.subscription_ids.length === 0
            ? "text-muted-foreground"
            : "truncate"
        }
        title={typeof targets === "string" ? targets : undefined}
      >
        {targets}
      </span>
    </div>
  );
}
