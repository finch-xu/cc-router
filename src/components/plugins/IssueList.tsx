import { useT } from "@/i18n";
import type { Issue } from "@/plugins/manifest";

/** 校验问题列表: 位置 + 一句按 code 翻译的说明 (+ 作者看的英文补充) */
export function IssueList({ issues }: { issues: Issue[] }) {
  const { t } = useT();
  return (
    <ul className="pg-issues">
      {issues.slice(0, 12).map((i, n) => (
        <li key={n}>
          {i.path && <code>{i.path}</code>} {t(`plugins.issue.${i.code}`)}
          {i.detail && <span className="mono" style={{ color: "var(--ink-4)" }}> ({i.detail})</span>}
        </li>
      ))}
      {issues.length > 12 && <li>…</li>}
    </ul>
  );
}
