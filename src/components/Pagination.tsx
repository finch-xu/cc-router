import { useT } from "@/i18n";

interface Props {
  total: number;
  page: number;
  totalPages: number;
  onChange: (page: number) => void;
  trailing?: React.ReactNode;
}

export function Pagination({ total, page, totalPages, onChange, trailing }: Props) {
  const { t } = useT();
  if (total <= 0) return null;
  return (
    <div
      style={{
        display: "flex",
        justifyContent: "flex-end",
        gap: 8,
        marginTop: 16,
        alignItems: "center",
      }}
    >
      <span
        className="mono"
        style={{ marginRight: "auto", fontSize: 12, color: "var(--ink-3)" }}
      >
        {t("requestLogs.summaryFormat", { total, page, pages: totalPages })}
      </span>
      {trailing}
      <button
        className="btn sm"
        disabled={page <= 1}
        onClick={() => onChange(Math.max(1, page - 1))}
        type="button"
      >
        {t("requestLogs.prevPage")}
      </button>
      <button
        className="btn sm"
        disabled={page >= totalPages}
        onClick={() => onChange(page + 1)}
        type="button"
      >
        {t("requestLogs.nextPage")}
      </button>
    </div>
  );
}
