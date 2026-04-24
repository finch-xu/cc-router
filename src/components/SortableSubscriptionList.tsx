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
import { Button } from "@/components/ui/button";
import { StatusBadge } from "@/components/StatusBadge";
import { ProviderIcon } from "@/components/ProviderIcon";
import { useProviders } from "@/hooks/useProviders";
import type { SubscriptionDto, SubscriptionSlot } from "@/types";

interface Props {
  subscriptionIds: string[];
  subscriptions: Map<string, SubscriptionDto>;
  /** null 表示 fallback 模式: 订阅会原样透传请求 model，不走 slot 映射 */
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
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 4 } }));
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
    return (
      <div className="rounded-md border border-dashed p-4 text-center text-sm text-muted-foreground">
        暂无订阅，点击下方按钮添加
      </div>
    );
  }

  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
      <SortableContext items={subscriptionIds} strategy={verticalListSortingStrategy}>
        <div className="space-y-1.5">
          {subscriptionIds.map((id) => {
            const sub = subscriptions.get(id);
            const realModel =
              slot === null
                ? "原样透传请求的 model"
                : sub
                  ? sub.model_slots[slot]
                  : "?";
            return (
              <SortableRow
                key={id}
                id={id}
                sub={sub}
                iconId={iconOf(sub?.provider_id)}
                realModel={realModel}
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
  sub,
  iconId,
  realModel,
  onRemove,
}: {
  id: string;
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

  return (
    <div
      ref={setNodeRef}
      style={style}
      className="flex items-center gap-3 rounded-md border bg-card px-3 py-2"
    >
      <button
        className="cursor-grab text-muted-foreground hover:text-foreground"
        {...attributes}
        {...listeners}
      >
        <GripVertical className="h-4 w-4" />
      </button>
      {sub ? (
        <StatusBadge state={sub.state} showLabel={false} />
      ) : (
        <span className="h-2 w-2 rounded-full bg-muted" />
      )}
      <ProviderIcon iconId={iconId} size={18} />
      <div className="flex-1 min-w-0">
        <div className="text-sm font-medium truncate">{sub?.display_name ?? "(未找到)"}</div>
        <div className="text-xs text-muted-foreground font-mono truncate">→ {realModel}</div>
      </div>
      <Button variant="ghost" size="sm" onClick={onRemove}>
        <X className="h-3.5 w-3.5" />
        移除
      </Button>
    </div>
  );
}
