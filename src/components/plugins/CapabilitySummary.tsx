import { useT } from "@/i18n";
import type { PluginCapabilities, PluginCapabilityDiff } from "@/types";

/**
 * 系统插件「会做什么」: 由 Rust 从插件内容推导 (不是作者自己写的, 没法少报)。
 * 厂商插件 = API Key 会发往哪些 host; 更新时再列出与已装版本的差异。
 */
export function CapabilitySummary({ capabilities, diff }: { capabilities: PluginCapabilities; diff?: PluginCapabilityDiff }) {
  const { t } = useT();
  const changed = diff && (diff.added_hosts.length > 0 || diff.removed_hosts.length > 0 || diff.auth_type_changed);
  return (
    <div className="pg-caps">
      <div className="pg-caps-title">{t("plugins.caps.keyHosts")}</div>
      <ul className="pg-caps-list">
        {capabilities.key_hosts.map((h) => (
          <li key={h.host}>
            <code>{h.host}</code>
            <span className="pg-caps-use">{h.used_for.map((u) => t(`plugins.caps.use.${u}`)).join(" · ")}</span>
            {h.differs_from_messages && <span className="pg-badge warn">{t("plugins.caps.differs")}</span>}
          </li>
        ))}
      </ul>
      {changed && (
        <div className="pg-alert warn">
          {t("plugins.caps.diffTitle")}
          <ul className="pg-issues">
            {diff.added_hosts.map((h) => (
              <li key={`+${h}`}>{t("plugins.caps.added", { host: h })}</li>
            ))}
            {diff.removed_hosts.map((h) => (
              <li key={`-${h}`}>{t("plugins.caps.removed", { host: h })}</li>
            ))}
            {diff.auth_type_changed && <li>{t("plugins.caps.authChanged")}</li>}
          </ul>
        </div>
      )}
    </div>
  );
}
