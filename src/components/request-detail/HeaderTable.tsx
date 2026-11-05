import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { useT } from "@/i18n";
import type { HeaderPairs } from "@/types";

export interface HeaderTag {
  label: string;
  tone: "ok" | "accent" | "muted";
}

/**
 * 一组请求头的两列表格 (名称 / 值)。表头 sticky, 值可折行; 右上角复制为 `name: value` 多行。
 * `pairs === null` 表示这条日志没有记录请求头 (老日志或当时关了记录)。
 */
export function HeaderTable({
  title,
  subtitle,
  pairs,
  loading,
  tagFor,
  emptyText,
}: {
  title: string;
  subtitle?: string;
  pairs: HeaderPairs | undefined;
  loading: boolean;
  tagFor?: (name: string, value: string) => HeaderTag | null;
  /** pairs 为 null 时显示的说明 (未记录 / 没发出 / 没收到响应 / 加载失败), 由调用方按整行情况决定 */
  emptyText: string;
}) {
  const { t } = useT();
  const [copied, setCopied] = useState(false);

  async function copy() {
    if (!pairs) return;
    try {
      await navigator.clipboard.writeText(pairs.map(([k, v]) => `${k}: ${v}`).join("\n"));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // ignore
    }
  }

  return (
    <section>
      <h3 className="rd-pane-title">
        <span>{title}</span>
        {subtitle && <span className="rd-pane-sub">{subtitle}</span>}
        {pairs && pairs.length > 0 && (
          <button className="btn sm rd-pane-action" type="button" onClick={copy}>
            {copied ? <Check size={12} /> : <Copy size={12} />}
            {copied ? t("common.copied") : t("common.copy")}
          </button>
        )}
      </h3>
      {loading ? (
        <div className="rd-empty">{t("common.loading")}</div>
      ) : !pairs ? (
        <div className="rd-empty">{emptyText}</div>
      ) : pairs.length === 0 ? (
        <div className="rd-empty">{t("requestLogs.detail.headers.empty")}</div>
      ) : (
        <table className="rd-table">
          <thead>
            <tr>
              <th>{t("requestLogs.detail.headers.name")}</th>
              <th>{t("requestLogs.detail.headers.value")}</th>
            </tr>
          </thead>
          <tbody>
            {pairs.map(([k, v], i) => {
              const tag = tagFor?.(k, v) ?? null;
              return (
                <tr key={`${k}-${i}`}>
                  <td className="rd-table-k">
                    {k}
                    {tag && <span className={`rd-tag ${tag.tone}`}>{tag.label}</span>}
                  </td>
                  <td className="rd-table-v">{v}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}
