import { useMemo, useState } from "react";
import { Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { StatusBadge } from "@/components/StatusBadge";
import { SortableSubscriptionList } from "@/components/SortableSubscriptionList";
import { RouteFlowDiagram } from "@/components/RouteFlowDiagram";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useVirtualModels, useUpdateVirtualModel } from "@/hooks/useVirtualModels";
import type { RoutingMode, SubscriptionDto, VirtualModelDto, VirtualModelName } from "@/types";

const VM_LABEL: Record<VirtualModelName, string> = {
  "model-opus": "高级任务 / Plan Mode",
  "model-sonnet": "主对话",
  "model-haiku": "小任务 / 工具调用",
  "model-fallback": "兜底 · 未知模型透传",
};

export function VirtualModelsPage() {
  const subs = useSubscriptions();
  const vms = useVirtualModels();

  const subsMap = useMemo(() => {
    const m = new Map<string, SubscriptionDto>();
    subs.data?.forEach((s) => m.set(s.id, s));
    return m;
  }, [subs.data]);

  return (
    <div className="p-8 space-y-6">
      <div>
        <h1 className="text-2xl font-semibold">虚拟模型</h1>
        <p className="text-sm text-muted-foreground">
          三个固定虚拟模型对应 Claude Code 的模型槽位；model-fallback 是兜底，任何其他 model 请求都走这里
        </p>
      </div>

      <RouteFlowDiagram />

      <div className="grid grid-cols-1 xl:grid-cols-2 gap-4">
        {vms.data?.map((vm) => (
          <VirtualModelCard
            key={vm.name}
            vm={vm}
            subsMap={subsMap}
            allSubs={subs.data ?? []}
          />
        ))}
      </div>
    </div>
  );
}

function VirtualModelCard({
  vm,
  subsMap,
  allSubs,
}: {
  vm: VirtualModelDto;
  subsMap: Map<string, SubscriptionDto>;
  allSubs: SubscriptionDto[];
}) {
  const updateMut = useUpdateVirtualModel();
  const [pickerOpen, setPickerOpen] = useState(false);
  const vmName = vm.name as VirtualModelName;

  function update(mode: RoutingMode, subscription_ids: string[]) {
    updateMut.mutate({ name: vmName, input: { mode, subscription_ids } });
  }

  function onReorder(ids: string[]) {
    update(vm.mode, ids);
  }
  function onRemove(id: string) {
    update(
      vm.mode,
      vm.subscription_ids.filter((x) => x !== id),
    );
  }
  function addSubs(ids: string[]) {
    const existing = new Set(vm.subscription_ids);
    const merged = [...vm.subscription_ids, ...ids.filter((id) => !existing.has(id))];
    update(vm.mode, merged);
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-3">
          <span className="font-mono text-base">{vm.name}</span>
          <span className="text-xs text-muted-foreground font-normal">{VM_LABEL[vmName]}</span>
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex items-center gap-6 text-sm">
          <Label>调度模式:</Label>
          <label className="inline-flex items-center gap-1.5 cursor-pointer">
            <input
              type="radio"
              name={`mode-${vm.name}`}
              checked={vm.mode === "sequential"}
              onChange={() => update("sequential", vm.subscription_ids)}
              className="accent-primary"
            />
            顺序
          </label>
          <label className="inline-flex items-center gap-1.5 cursor-pointer">
            <input
              type="radio"
              name={`mode-${vm.name}`}
              checked={vm.mode === "round_robin"}
              onChange={() => update("round_robin", vm.subscription_ids)}
              className="accent-primary"
            />
            轮询
          </label>
        </div>

        <SortableSubscriptionList
          subscriptionIds={vm.subscription_ids}
          subscriptions={subsMap}
          slot={
            vmName === "model-opus"
              ? "opus"
              : vmName === "model-sonnet"
                ? "sonnet"
                : vmName === "model-haiku"
                  ? "haiku"
                  : null
          }
          onChange={onReorder}
          onRemove={onRemove}
        />

        <Button variant="outline" size="sm" onClick={() => setPickerOpen(true)}>
          <Plus className="h-3.5 w-3.5" /> 添加订阅到此虚拟模型
        </Button>
      </CardContent>

      <AddSubscriptionDialog
        open={pickerOpen}
        onOpenChange={setPickerOpen}
        existingIds={vm.subscription_ids}
        allSubs={allSubs}
        onConfirm={(ids) => {
          addSubs(ids);
          setPickerOpen(false);
        }}
      />
    </Card>
  );
}

function AddSubscriptionDialog({
  open,
  onOpenChange,
  existingIds,
  allSubs,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  existingIds: string[];
  allSubs: SubscriptionDto[];
  onConfirm: (ids: string[]) => void;
}) {
  const [selected, setSelected] = useState<Set<string>>(new Set());

  const candidates = allSubs.filter((s) => s.enabled && !existingIds.includes(s.id));

  function toggle(id: string) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>选择要添加的订阅</DialogTitle>
        </DialogHeader>
        {candidates.length === 0 ? (
          <p className="text-sm text-muted-foreground">没有可用的订阅。先到「订阅管理」添加一个。</p>
        ) : (
          <div className="space-y-1">
            {candidates.map((sub) => (
              <label
                key={sub.id}
                className="flex items-center gap-3 rounded-md px-2 py-2 hover:bg-muted cursor-pointer"
              >
                <input
                  type="checkbox"
                  checked={selected.has(sub.id)}
                  onChange={() => toggle(sub.id)}
                  className="accent-primary"
                />
                <StatusBadge state={sub.state} showLabel={false} />
                <span className="text-sm flex-1">{sub.display_name}</span>
                {sub.state === "auth_failed" && (
                  <span className="text-xs text-destructive">⚠️ 凭证失效</span>
                )}
              </label>
            ))}
          </div>
        )}
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button disabled={selected.size === 0} onClick={() => onConfirm(Array.from(selected))}>
            添加所选
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
