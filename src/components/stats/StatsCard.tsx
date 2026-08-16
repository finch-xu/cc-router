import type { ReactNode } from "react";

/**
 * 统计页卡片壳: 标题 / 副标题 / 右上角操作位 / 空态 / 加载态 (保留旧图降透明度, 不闪骨架) / 错误态。
 * 复用 `.stats-section` (小票页也用它, 定义不动), 新增的状态 class 只挂在本组件。
 */
export function StatsCard({
  title,
  subtitle,
  right,
  isEmpty,
  emptyText,
  loading,
  errorText,
  className,
  children,
}: {
  title: string;
  subtitle?: string;
  right?: ReactNode;
  isEmpty: boolean;
  emptyText: string;
  loading?: boolean;
  errorText?: string | null;
  className?: string;
  children: ReactNode;
}) {
  return (
    <section
      className={
        "stats-section" +
        (loading ? " is-loading" : "") +
        (className ? " " + className : "")
      }
    >
      <div className="stats-section-header stats-card-head">
        <div>
          <div className="stats-section-title">{title}</div>
          {subtitle && <div className="stats-section-subtitle">{subtitle}</div>}
        </div>
        {right && <div className="stats-card-right">{right}</div>}
      </div>
      <div className="stats-body">
        {errorText ? (
          <div className="field-hint stats-error">{errorText}</div>
        ) : isEmpty ? (
          <div className="field-hint">{emptyText}</div>
        ) : (
          children
        )}
      </div>
    </section>
  );
}
