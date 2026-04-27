import { useMemo, useState } from "react";
import { Plus } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { StatusDot, stateLabel } from "@/components/StatusBadge";
import { SortableSubscriptionList } from "@/components/SortableSubscriptionList";
import { RouteFlowDiagram } from "@/components/RouteFlowDiagram";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useVirtualModels, useUpdateVirtualModel } from "@/hooks/useVirtualModels";
import { VM_META, vmNameToSlot } from "@/lib/virtualModels";
import { useT } from "@/i18n";
import type { RoutingMode, SubscriptionDto, VirtualModelDto } from "@/types";

export function VirtualModelsPage() {
  const { t } = useT();
  const subs = useSubscriptions();
  const vms = useVirtualModels();

  const subsMap = useMemo(() => {
    const m = new Map<string, SubscriptionDto>();
    subs.data?.forEach((s) => m.set(s.id, s));
    return m;
  }, [subs.data]);

  return (
    <>
      <div className="page-header">
        <h1>{t("virtualModels.title")}</h1>
        <div className="subtitle">
          {t("virtualModels.subtitle1")}
          <span className="mono" style={{ color: "var(--ink-2)" }}> model-fallback</span>
          {t("virtualModels.subtitle2")}
        </div>
      </div>

      <RouteFlowDiagram />

      <div className="slot-grid">
        {vms.data?.map((vm) => (
          <VirtualModelCard
            key={vm.name}
            vm={vm}
            subsMap={subsMap}
            allSubs={subs.data ?? []}
          />
        ))}
      </div>
    </>
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
  const { t } = useT();
  const updateMut = useUpdateVirtualModel();
  const [pickerOpen, setPickerOpen] = useState(false);
  const meta = VM_META[vm.name];

  function update(mode: RoutingMode, subscription_ids: string[]) {
    updateMut.mutate({ name: vm.name, input: { mode, subscription_ids } });
  }
  function onReorder(ids: string[]) {
    update(vm.mode, ids);
  }
  function onRemove(id: string) {
    update(vm.mode, vm.subscription_ids.filter((x) => x !== id));
  }
  function addSubs(ids: string[]) {
    const existing = new Set(vm.subscription_ids);
    const merged = [...vm.subscription_ids, ...ids.filter((id) => !existing.has(id))];
    update(vm.mode, merged);
  }

  const slot = vmNameToSlot(vm.name);
  const modeHint =
    vm.mode === "round_robin"
      ? t("virtualModels.mode.roundRobinHint")
      : t("virtualModels.mode.sequentialHint");

  return (
    <div className="slot-card">
      <div className="slot-head">
        <div>
          <span className="slot-name">{vm.name}</span>
          <span className="slot-purpose">
            <strong>{t(meta.purposeKey)}</strong> · {t(meta.purposeEnKey)}
          </span>
        </div>
        <span className="pill accent">
          <span className="dot" />
          {vm.subscription_ids.length}{t("virtualModels.endpointsSuffix")}
        </span>
      </div>

      <div className="slot-mode-row">
        <span style={{ fontWeight: 500, color: "var(--ink-2)" }}>{t("virtualModels.mode.label")}</span>
        <div className="radio-group">
          <button
            className={vm.mode === "sequential" ? "on" : ""}
            onClick={() => update("sequential", vm.subscription_ids)}
            type="button"
          >
            {t("vm.mode.sequential")}
          </button>
          <button
            className={vm.mode === "round_robin" ? "on" : ""}
            onClick={() => update("round_robin", vm.subscription_ids)}
            type="button"
          >
            {t("vm.mode.round_robin")}
          </button>
        </div>
        <span
          className="mono"
          style={{ marginLeft: "auto", fontSize: 11.5, color: "var(--ink-4)" }}
        >
          {modeHint}
        </span>
      </div>

      <SortableSubscriptionList
        subscriptionIds={vm.subscription_ids}
        subscriptions={subsMap}
        slot={slot}
        vmName={vm.name}
        onChange={onReorder}
        onRemove={onRemove}
      />

      <button className="add-endpoint" onClick={() => setPickerOpen(true)} type="button">
        <Plus size={12} /> {t("virtualModels.addButton")}
      </button>

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
    </div>
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
  const { t } = useT();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const candidates = allSubs.filter((s) => s.enabled && !existingIds.includes(s.id));

  function toggle(id: string) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(v) => {
        if (!v) setSelected(new Set());
        onOpenChange(v);
      }}
    >
      <DialogContent className="cc-dialog">
        <DialogHeader>
          <DialogTitle>{t("virtualModels.dialog.title")}</DialogTitle>
        </DialogHeader>
        {candidates.length === 0 ? (
          <div className="field-hint">{t("virtualModels.dialog.empty")}</div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            {candidates.map((sub) => (
              <label
                key={sub.id}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 12,
                  padding: "8px 12px",
                  borderRadius: 6,
                  cursor: "pointer",
                  background: selected.has(sub.id) ? "var(--surface-2)" : "transparent",
                  border: "1px solid " + (selected.has(sub.id) ? "var(--line)" : "transparent"),
                }}
                onMouseEnter={(e) => {
                  if (!selected.has(sub.id)) e.currentTarget.style.background = "var(--surface-2)";
                }}
                onMouseLeave={(e) => {
                  if (!selected.has(sub.id)) e.currentTarget.style.background = "transparent";
                }}
              >
                <input
                  type="checkbox"
                  checked={selected.has(sub.id)}
                  onChange={() => toggle(sub.id)}
                  style={{ accentColor: "var(--ink)" }}
                />
                <StatusDot state={sub.state} />
                <span style={{ fontSize: 13, flex: 1 }}>{sub.display_name}</span>
                {sub.state === "auth_failed" && (
                  <span className="pill err">{stateLabel("auth_failed", t)}</span>
                )}
              </label>
            ))}
          </div>
        )}
        <DialogFooter>
          <button className="btn" onClick={() => onOpenChange(false)} type="button">
            {t("common.cancel")}
          </button>
          <button
            className="btn primary"
            disabled={selected.size === 0}
            onClick={() => onConfirm(Array.from(selected))}
            type="button"
          >
            {t("virtualModels.dialog.add")}
          </button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
