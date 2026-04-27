import {
  DndContext,
  closestCenter,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  useSortable,
  verticalListSortingStrategy,
  arrayMove,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical } from "lucide-react";
import { ProviderLogo } from "@/components/ProviderLogo";
import { stateTone } from "@/components/StatusBadge";
import { useRouteFlashState } from "@/hooks/useRouteFlash";
import { useT } from "@/i18n";
import type { SubscriptionDto, SubscriptionSlot, VirtualModelName } from "@/types";

interface Props {
  subscriptionIds: string[];
  subscriptions: Map<string, SubscriptionDto>;
  /** null 表示 fallback 模式: 订阅会原样透传请求 model,不走 slot 映射 */
  slot: SubscriptionSlot | null;
  /** 用于关联实时路由事件: 同一订阅在不同 vm 槽位下独立闪烁 */
  vmName: VirtualModelName;
  onChange: (ids: string[]) => void;
  onRemove: (id: string) => void;
}

export function SortableSubscriptionList({
  subscriptionIds,
  subscriptions,
  slot,
  vmName,
  onChange,
  onRemove,
}: Props) {
  const { t } = useT();
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
  );

  function handleDragEnd(event: DragEndEvent) {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const oldIndex = subscriptionIds.indexOf(String(active.id));
    const newIndex = subscriptionIds.indexOf(String(over.id));
    if (oldIndex < 0 || newIndex < 0) return;
    onChange(arrayMove(subscriptionIds, oldIndex, newIndex));
  }

  if (subscriptionIds.length === 0) {
    return <div className="endpoint-empty">{t("sortableSub.empty")}</div>;
  }

  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
      <SortableContext items={subscriptionIds} strategy={verticalListSortingStrategy}>
        {subscriptionIds.map((id, idx) => {
          const sub = subscriptions.get(id);
          const realModel =
            slot === null ? t("sortableSub.passthrough") : sub ? sub.model_slots[slot] : "?";
          return (
            <SortableRow
              key={id}
              id={id}
              vmName={vmName}
              priority={idx + 1}
              sub={sub}
              iconId={sub?.provider_icon}
              realModel={realModel}
              onRemove={() => onRemove(id)}
            />
          );
        })}
      </SortableContext>
    </DndContext>
  );
}

function SortableRow({
  id,
  vmName,
  priority,
  sub,
  iconId,
  realModel,
  onRemove,
}: {
  id: string;
  vmName: VirtualModelName;
  priority: number;
  sub: SubscriptionDto | undefined;
  iconId: string | undefined;
  realModel: string;
  onRemove: () => void;
}) {
  const { t } = useT();
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
  const flash = useRouteFlashState(vmName, id);
  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.6 : 1,
  };

  const tone = sub ? stateTone(sub.state) : "neutral";
  const dotClass =
    tone === "ok" ? "" : tone === "err" ? " err" : tone === "warn" ? " warn" : " idle";
  const flashClass = flash ? ` route-flash-${flash.kind}` : "";

  return (
    <div ref={setNodeRef} style={style} className={`endpoint${flashClass}`}>
      <button className="grip" {...attributes} {...listeners} type="button" aria-label={t("sortableSub.dragHandle")}>
        <GripVertical size={14} strokeWidth={1.6} />
      </button>
      <span className="priority mono">{priority}</span>
      <ProviderLogo iconId={iconId} size={22} />
      <div className="endpoint-info">
        <div className="endpoint-name">
          {sub?.display_name ?? t("common.notFound")}
          <span className={`endpoint-status${dotClass}`} aria-hidden />
        </div>
        <div className="endpoint-model mono">{realModel}</div>
      </div>
      <button className="remove" onClick={onRemove} type="button">
        {t("sortableSub.remove")}
      </button>
    </div>
  );
}
