import { useState } from "react";
import { Navigate } from "react-router";
import { ChevronDown, Ellipsis } from "lucide-react";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import ClaudeCode from "@lobehub/icons/es/ClaudeCode";
import Codex from "@lobehub/icons/es/Codex";
import { useT } from "@/i18n";
import { isTauriHost, runtime } from "@/runtime";
import { agentNames, useAgentMonitor, useAgentMonitorBusy, useAgentVersion, useConfigureAgentMonitor } from "@/hooks/useAgentMonitor";

const agentIcons = { claude: ClaudeCode.Color, codex: Codex };

export function AgentMonitorPage() {
  return isTauriHost ? <DesktopAgentMonitor /> : <Navigate to="/live-routing" replace />;
}

function DesktopAgentMonitor() {
  const { t } = useT();
  const connections = useAgentMonitor(3_000);
  const configure = useConfigureAgentMonitor();
  const mutating = useAgentMonitorBusy();
  return (
    <>
      <div className="page-header">
        <h1>{t("agentMonitor.title")}</h1>
        <div className="subtitle">{t("agentMonitor.subtitle")}</div>
      </div>
      {connections.isPending && <p role="status">{t("agentMonitor.loading")}</p>}
      {connections.isError && <p role="alert">{t("agentMonitor.loadFailed")}</p>}
      {connections.data && (
        <section className="card agent-monitor">
          {connections.data.map((row) => (
            <AgentRow key={row.client} row={row} configure={configure} mutating={mutating} />
          ))}
        </section>
      )}
    </>
  );
}

type Row = NonNullable<ReturnType<typeof useAgentMonitor>["data"]>[number];

function AgentRow({ row, configure, mutating }: {
  row: Row;
  configure: ReturnType<typeof useConfigureAgentMonitor>;
  mutating: boolean;
}) {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const version = useAgentVersion(row.client, open);
  const name = agentNames[row.client];
  const Icon = agentIcons[row.client];
  const enabled = row.connection === "enabled";
  const unconfigured = row.connection === "unconfigured";
  const cleanup = row.connection === "cleanup_pending";
  const waiting = enabled && row.last_report_ms === null;
  const busy = configure.isPending && configure.variables.client === row.client;
  const error = row.error;
  const detailsId = `agent-monitor-${row.client}`;
  const status = waiting ? "waiting" : row.connection;
  const action = cleanup ? "clear" : enabled ? "pause" : "enable";
  const label = cleanup ? "retry" : enabled ? "pause" : unconfigured ? "connect" : "enable";

  async function clear() {
    if (await runtime.confirm(t("agentMonitor.confirmClear", { client: name }))) {
      configure.mutate({ client: row.client, action: "clear" });
    }
  }

  return (
    <article className="agent-monitor-entry">
      <div className="agent-monitor-row">
        <button className="agent-monitor-name" type="button" aria-expanded={open} aria-controls={detailsId} onClick={() => setOpen(!open)}>
          <ChevronDown size={14} className={open ? "open" : undefined} aria-hidden="true" />
          <Icon size={20} aria-hidden="true" />
          <strong>{name}</strong>
        </button>
        <span className="agent-monitor-state" data-state={status}>{t(`agentMonitor.state.${status}`)}</span>
        <span className="agent-monitor-report">
          {row.last_report_ms === null
            ? t("agentMonitor.noReport")
            : <time dateTime={new Date(row.last_report_ms).toISOString()}>
                {t("agentMonitor.lastReport", { time: new Date(row.last_report_ms).toLocaleString() })}
              </time>}
        </span>
        <div className="agent-monitor-actions">
          <button type="button" disabled={mutating} onClick={() => configure.mutate({ client: row.client, action })}>
            {busy ? t("agentMonitor.working") : t(`agentMonitor.${label}`)}
          </button>
          {!unconfigured && !cleanup && (
            <DropdownMenu.Root>
              <DropdownMenu.Trigger
                disabled={mutating}
                aria-label={t("agentMonitor.more", { client: name })}
              >
                <Ellipsis size={17} aria-hidden="true" />
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal>
                <DropdownMenu.Content className="agent-monitor-menu" align="end" sideOffset={5}>
                  <DropdownMenu.Item onSelect={clear}>{t("agentMonitor.clear")}</DropdownMenu.Item>
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>
          )}
        </div>
      </div>
      {waiting && <p className="agent-monitor-note">{t(row.client === "codex" ? "agentMonitor.codexSetup" : "agentMonitor.claudeSetup")}</p>}
      {error && <p role="alert" className="agent-monitor-error">{error}</p>}
      <div id={detailsId} hidden={!open} className="agent-monitor-details">
        <dl>
          <div><dt>{t("agentMonitor.clientVersion")}</dt><dd>{version.data ?? t("liveSessions.unavailable")}</dd></div>
          <div><dt>{t("agentMonitor.pluginVersion")}</dt><dd>{row.plugin_version ?? t("liveSessions.unavailable")}</dd></div>
          <div><dt>{t("agentMonitor.availableData")}</dt><dd>{t("agentMonitor.dataTypes")}</dd></div>
        </dl>
      </div>
    </article>
  );
}
