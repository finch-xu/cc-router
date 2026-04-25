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
import { useProviders } from "@/hooks/useProviders";
import type { SubscriptionDto, SubscriptionSlot } from "@/types";

interface Props {
  subscriptionIds: string[];
  subscriptions: Map<string, SubscriptionDto>;
  /** null 表示 fallback 模式: 订阅会原样透传请求 model,不走 slot 映射 */
  slot: SubscriptionSlot | null;
  onChange: (ids: string[]) => void;
  onRemove: (id: string) => void;
}

export function SortableSubscriptionList({
  subscriptionIds,
  subscriptions,
  slot,
  onChange,
  onRemove,
}: Props) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
  );
  const providers = useProviders();
  const iconOf = (providerId: string | undefined) =>
    providerId ? providers.data?.find((p) => p.id === providerId)?.icon : undefined;

  function handleDragEnd(event: DragEndEvent) {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const oldIndex = subscriptionIds.indexOf(String(active.id));
    const newIndex = subscriptionIds.indexOf(String(over.id));
    if (oldIndex < 0 || newIndex < 0) return;
    onChange(arrayMove(subscriptionIds, oldIndex, newIndex));
  }

  if (subscriptionIds.length === 0) {
    return <div className="endpoint-empty">暂无订阅,点击下方按钮添加</div>;
  }

  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
      <SortableContext items={subscriptionIds} strategy={verticalListSortingStrategy}>
        {subscriptionIds.map((id, idx) => {
          const sub = subscriptions.get(id);
          const realModel =
            slot === null ? "原样透传请求的 model" : sub ? sub.model_slots[slot] : "?";
          return (
            <SortableRow
              key={id}
              id={id}
              priority={idx + 1}
              sub={sub}
              iconId={iconOf(sub?.provider_id)}
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
  priority,
  sub,
  iconId,
  realModel,
  onRemove,
}: {
  id: string;
  priority: number;
  sub: SubscriptionDto | undefined;
  iconId: string | undefined;
  realModel: string;
  onRemove: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.6 : 1,
  };

  const tone = sub ? stateTone(sub.state) : "neutral";
  const dotClass =
    tone === "ok" ? "" : tone === "err" ? " err" : tone === "warn" ? " warn" : " idle";

  return (
    <div ref={setNodeRef} style={style} className="endpoint">
      <button className="grip" {...attributes} {...listeners} type="button" aria-label="拖拽排序">
        <GripVertical size={14} strokeWidth={1.6} />
      </button>
      <span className="priority mono">{priority}</span>
      <ProviderLogo iconId={iconId} size={22} />
      <div className="endpoint-info">
        <div className="endpoint-name">
          {sub?.display_name ?? "(未找到)"}
          <span className={`endpoint-status${dotClass}`} aria-hidden />
        </div>
        <div className="endpoint-model mono">{realModel}</div>
      </div>
      <button className="remove" onClick={onRemove} type="button">
        移除
      </button>
    </div>
  );
}
