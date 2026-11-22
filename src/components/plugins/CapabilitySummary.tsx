import { useT } from "@/i18n";
import type { PluginCapabilities, PluginCapabilityDiff } from "@/types";

const VIRTUAL_MODEL_LABEL: Record<string, string> = {
  fable: "model-fable",
  opus: "model-opus",
  sonnet: "model-sonnet",
  haiku: "model-haiku",
  fallback: "model-fallback",
};

/**
 * 系统插件「会做什么」: 由 Rust 从插件内容推导 (不是作者自己写的, 没法少报)。
 * 厂商插件 = API Key 会发往哪些 host; 计价插件 = 币种、条目数、覆盖的厂商;
 * 规则插件 = 用到的钩子、动作、可能改道到的虚拟模型。
 */
export function CapabilitySummary({ capabilities, diff }: { capabilities: PluginCapabilities; diff?: PluginCapabilityDiff }) {
  const { t } = useT();
  if (capabilities.kind === "pricing") {
    // 计价插件不连网也不改请求, 只说明它覆盖什么
    return (
      <div className="pg-caps">
        <div className="pg-caps-title">
          {t("plugins.caps.pricing", { currency: capabilities.currency, rules: capabilities.rules })}
        </div>
        <ul className="pg-caps-list">
          {capabilities.providers.map((p) => (
            <li key={p}>
              <code>{p === "*" ? t("plugins.caps.anyProvider") : p}</code>
            </li>
          ))}
        </ul>
      </div>
    );
  }
  if (capabilities.kind === "rules") {
    const added = diff?.added_actions ?? [];
    const actionLabel = (a: string) => {
      if (a.startsWith("hook:")) {
        const hk = a.slice(5);
        const key = `plugins.caps.hook.${hk}`;
        const label = t(key);
        return label === key ? a : label;
      }
      const key = `plugins.caps.action.${a}`;
      const label = t(key);
      return label === key ? a : label;
    };
    return (
      <div className="pg-caps">
        <div className="pg-caps-title">{t("plugins.caps.rules", { rules: capabilities.rules })}</div>
        <dl className="pg-facts pg-caps-dl">
          <dt>{t("plugins.caps.hooks")}</dt>
          <dd className="pg-caps-tags">
            {capabilities.hooks.map((h) => (
              <span key={h} className="pg-tag">{t(`plugins.caps.hook.${h}`)}</span>
            ))}
          </dd>
          <dt>{t("plugins.caps.actions")}</dt>
          <dd className="pg-caps-tags">
            {capabilities.actions.map((a) => (
              <span key={a} className="pg-tag">{actionLabel(a)}</span>
            ))}
          </dd>
          {capabilities.redirects_to.length > 0 && (
            <>
              <dt>{t("plugins.caps.redirects")}</dt>
              <dd className="pg-caps-tags">
                {capabilities.redirects_to.map((r) => (
                  <code key={r}>{r.startsWith("{{") ? t("plugins.caps.byParam") : (VIRTUAL_MODEL_LABEL[r] ?? r)}</code>
                ))}
              </dd>
            </>
          )}
        </dl>
        {added.length > 0 && (
          <div className="pg-alert warn">
            {t("plugins.caps.diffTitle")}
            <ul className="pg-issues">
              {added.map((a) => (
                <li key={a}>{t("plugins.caps.addedAction", { action: actionLabel(a) })}</li>
              ))}
            </ul>
          </div>
        )}
      </div>
    );
  }
  if (capabilities.kind === "notifier") {
    const addedHosts = diff?.added_hosts ?? [];
    const removedHosts = diff?.removed_hosts ?? [];
    const addedEvents = diff?.added_events ?? [];
    const addedFields = diff?.added_fields ?? [];
    const hasDiff = addedHosts.length > 0 || removedHosts.length > 0 || addedEvents.length > 0 || addedFields.length > 0;
    const eventLabel = (ev: string) => {
      const key = `notify.event.${ev}`;
      const label = t(key);
      return label === key ? ev : label;
    };
    return (
      <div className="pg-caps">
        <div className="pg-caps-title">{t("plugins.caps.notifier")}</div>
        <dl className="pg-facts pg-caps-dl">
          <dt>{t("plugins.caps.notifierHosts")}</dt>
          <dd className="pg-caps-tags">
            {capabilities.hosts.map((h) => (
              <code key={h}>{h}</code>
            ))}
            {capabilities.base_url_params.map((p) => (
              <span key={p} className="pg-tag">{t("plugins.caps.baseUrlParam", { param: p })}</span>
            ))}
          </dd>
          <dt>{t("plugins.caps.notifierEvents")}</dt>
          <dd className="pg-caps-tags">
            {capabilities.events.map((ev) => (
              <span key={ev} className="pg-tag">{eventLabel(ev)}</span>
            ))}
          </dd>
          {capabilities.fields.length > 0 && (
            <>
              <dt>{t("plugins.caps.notifierFields")}</dt>
              <dd className="pg-caps-tags">
                {capabilities.fields.map((f) => (
                  <code key={f}>{f}</code>
                ))}
              </dd>
            </>
          )}
        </dl>
        {hasDiff && (
          <div className="pg-alert warn">
            {t("plugins.caps.diffTitle")}
            <ul className="pg-issues">
              {addedHosts.map((h) => (
                <li key={`+h:${h}`}>{t("plugins.caps.added", { host: h })}</li>
              ))}
              {removedHosts.map((h) => (
                <li key={`-h:${h}`}>{t("plugins.caps.removed", { host: h })}</li>
              ))}
              {addedEvents.map((ev) => (
                <li key={`+e:${ev}`}>{t("plugins.caps.addedEvent", { event: eventLabel(ev) })}</li>
              ))}
              {addedFields.map((f) => (
                <li key={`+f:${f}`}>{t("plugins.caps.addedField", { field: f })}</li>
              ))}
            </ul>
          </div>
        )}
      </div>
    );
  }
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
