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
import { GripVertical, X } from "lucide-react";
import { ProviderLogo } from "@/components/ProviderLogo";
import { stateTone } from "@/components/StatusBadge";
import { useRouteFlashState } from "@/hooks/useRouteFlash";
import { useT } from "@/i18n";
import { isAnthropicPassthrough } from "@/lib/authTypes";
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
    return <div className="endpoint-empty compact">{t("sortableSub.empty")}</div>;
  }

  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
      <SortableContext items={subscriptionIds} strategy={verticalListSortingStrategy}>
        <div className="endpoint-list">
          {subscriptionIds.map((id, idx) => {
            const sub = subscriptions.get(id);
            // fallback 卡片三态: 配置了兜底槽 → 槽值; 未配置且透传类 → 「原样透传」;
            // 未配置且翻译类 → 警示 (dispatch 层会跳过该候选)。
            const fallbackModel = sub?.model_slots.fallback?.trim() ?? "";
            const realModel =
              slot === null
                ? fallbackModel || t("sortableSub.passthrough")
                : sub
                  ? sub.model_slots[slot]
                  : "?";
            const fallbackSkipped =
              slot === null && !fallbackModel && !!sub && !isAnthropicPassthrough(sub.auth_type);
            return (
              <SortableRow
                key={id}
                id={id}
                vmName={vmName}
                priority={idx + 1}
                sub={sub}
                iconId={sub?.provider_icon}
                realModel={realModel}
                fallbackSkipped={fallbackSkipped}
                onRemove={() => onRemove(id)}
              />
            );
          })}
        </div>
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
  fallbackSkipped,
  onRemove,
}: {
  id: string;
  vmName: VirtualModelName;
  priority: number;
  sub: SubscriptionDto | undefined;
  iconId: string | undefined;
  realModel: string;
  /** fallback 卡片专用: 翻译类订阅未配置兜底槽, dispatch 会跳过 → 显示警示替代模型名 */
  fallbackSkipped?: boolean;
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
    <div ref={setNodeRef} style={style} className={`endpoint compact${flashClass}`}>
      <button className="grip" {...attributes} {...listeners} type="button" aria-label={t("sortableSub.dragHandle")}>
        <GripVertical size={12} strokeWidth={1.6} />
      </button>
      <span className="priority mono">{priority}</span>
      <ProviderLogo iconId={iconId} size={18} iconSize={11} />
      <div className="endpoint-info">
        <div className="endpoint-name">
          <span className={`endpoint-status${dotClass}`} aria-hidden />
          {sub?.display_name ?? t("common.notFound")}
        </div>
        {fallbackSkipped ? (
          <div className="endpoint-model">
            <span className="pill err">{t("sortableSub.fallbackSkipped")}</span>
          </div>
        ) : (
          <div className="endpoint-model mono">{realModel}</div>
        )}
      </div>
      <button className="remove" onClick={onRemove} type="button" aria-label={t("sortableSub.remove")}>
        <X size={12} strokeWidth={2} />
      </button>
    </div>
  );
}
