import type { ComponentType, ReactNode } from "react";

interface Props {
  icon: ComponentType<{ size?: number; className?: string; style?: React.CSSProperties }>;
  message: ReactNode;
  action?: ReactNode;
}

export function EmptyState({ icon: Icon, message, action }: Props) {
  return (
    <div className="card">
      <div className="empty-state">
        <Icon size={32} style={{ color: "var(--ink-4)" }} />
        <div className="field-hint" style={{ marginTop: 0 }}>
          {message}
        </div>
        {action}
      </div>
    </div>
  );
}
